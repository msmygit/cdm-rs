//! Tier 2: semantic validation — the cross-field rules of `SPEC` §3.4 (`CFG-030`..`CFG-040`).
//!
//! Everything here relates two or more properties to each other but still needs no cluster. The
//! rules that additionally need the live schema — `CFG-034`'s "resolvable" writetime column,
//! `CFG-036`, `CFG-038` and `CFG-039` — are completed in [Tier 3](super::tier3); what is checked
//! here is the part of each that is decidable without one, so that an obviously broken
//! configuration is rejected before a session is opened.

use cdm_core::Diagnostic;

use super::{error, notice, warning, ValidationOptions};
use crate::model::CdmConfig;

/// Runs every Tier-2 check.
pub(super) fn check(config: &CdmConfig, _options: ValidationOptions) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    constant_columns(config, &mut out);
    explode_map(config, &mut out);
    writetime_filter(config, &mut out);
    writetime_transform(config, &mut out);
    ttl_and_writetime_columns(config, &mut out);
    column_rename_syntax(config, &mut out);
    extract_json(config, &mut out);
    batch_size(config, &mut out);
    out
}

/// `CFG-030`: names and the split of values must have equal cardinality.
fn constant_columns(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let feature = &config.feature.constant_columns;
    let names = &feature.names;
    let values = feature.values.as_deref();

    match (names.is_empty(), values) {
        (true, None | Some("")) => return,
        (true, Some(values)) => {
            out.push(
                error(
                    "feature.constant_columns.names",
                    "constant column values were given without any column names",
                    "CFG-030",
                )
                .with_value(values.to_owned()),
            );
            return;
        }
        (false, None | Some("")) => {
            out.push(
                error(
                    "feature.constant_columns.values",
                    "constant column names were given without any values",
                    "CFG-030",
                )
                .with_value(names.join(",")),
            );
            return;
        }
        (false, Some(_)) => {}
    }

    let Some(values) = values else { return };
    // An unparseable split expression is Tier 1's complaint; do not repeat it here.
    let Ok(separator) = regex::Regex::new(&feature.split_regex) else {
        return;
    };
    let split: Vec<&str> = separator.split(values).collect();
    if split.len() != names.len() {
        out.push(
            error(
                "feature.constant_columns.values",
                "the constant column names and values differ in count",
                "CFG-030",
            )
            .with_value(values.to_owned())
            .with_detail(format!(
                "{} name(s) but {} value(s) after splitting on `{}`",
                names.len(),
                split.len(),
                feature.split_regex
            ))
            .with_suggestion(
                "set `feature.constant_columns.split_regex` if a value itself contains the \
                 separator",
            ),
        );
    }
}

/// `CFG-031`: explode map needs all three column names, or none.
fn explode_map(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let feature = &config.feature.explode_map;
    let present: Vec<&str> = [
        ("feature.explode_map.origin_column", &feature.origin_column),
        (
            "feature.explode_map.target_key_column",
            &feature.target_key_column,
        ),
        (
            "feature.explode_map.target_value_column",
            &feature.target_value_column,
        ),
    ]
    .into_iter()
    .filter_map(|(key, value)| {
        value
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
            .then_some(key)
    })
    .collect();

    if !present.is_empty() && present.len() != 3 {
        out.push(
            error(
                "feature.explode_map",
                "explode map needs all three column names, or none",
                "CFG-031",
            )
            .with_detail(format!(
                "only {} of 3 are set: {}",
                present.len(),
                present.join(", ")
            ))
            .with_suggestion(
                "set `origin_column`, `target_key_column` and `target_value_column`, or clear \
                 all three",
            ),
        );
    }
}

/// `CFG-032`: both writetime bounds must be positive, and the maximum must exceed the minimum.
/// `CFG-034`: a writetime filter needs at least one writetime column to filter on.
fn writetime_filter(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let filter = &config.filter.writetime;
    if filter.min.is_none() && filter.max.is_none() {
        return;
    }

    for (key, value) in [
        ("filter.writetime.min", filter.min),
        ("filter.writetime.max", filter.max),
    ] {
        match value {
            None => out.push(
                error(
                    key,
                    "a writetime filter needs both a minimum and a maximum",
                    "CFG-032",
                )
                .with_suggestion("set both bounds, or neither"),
            ),
            Some(bound) if bound <= 0 => out.push(
                error(
                    key,
                    "a writetime bound must be greater than zero",
                    "CFG-032",
                )
                .with_value(bound.to_string()),
            ),
            Some(_) => {}
        }
    }

    if let (Some(min), Some(max)) = (filter.min, filter.max) {
        if max <= min {
            out.push(
                error(
                    "filter.writetime.max",
                    "the writetime maximum must be greater than the minimum",
                    "CFG-032",
                )
                .with_value(format!("min={min}, max={max}")),
            );
        }
    }

    // CFG-034, the half that needs no schema: with automatic resolution off and no explicit
    // columns, there is nothing to filter on and no cluster can change that.
    let writetime = &config.schema.origin.writetime;
    if !writetime.automatic && writetime.names.is_empty() {
        out.push(
            error(
                "schema.origin.writetime.names",
                "a writetime filter needs at least one writetime column",
                "CFG-034",
            )
            .with_detail(
                "`schema.origin.writetime.automatic` is false and no columns are named, so no \
                 writetime can be resolved",
            )
            .with_suggestion(
                "name the columns in `schema.origin.writetime.names`, or re-enable automatic \
                 resolution",
            ),
        );
    }
}

/// `CFG-033`: the writetime increment may not be negative.
/// `CFG-039`, first half: an increment of zero is only a problem with an unfrozen list, which
/// Tier 3 checks; a zero increment on its own is perfectly normal.
fn writetime_transform(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let increment = config.transform.custom_writetime_increment;
    if increment < 0 {
        out.push(
            error(
                "transform.custom_writetime_increment",
                "the writetime increment may not be negative",
                "CFG-033",
            )
            .with_value(increment.to_string())
            .with_suggestion("use 0 to leave writetimes unchanged"),
        );
    }

    if config.transform.custom_writetime < 0 {
        out.push(
            error(
                "transform.custom_writetime",
                "a custom writetime may not be negative",
                "CFG-033",
            )
            .with_value(config.transform.custom_writetime.to_string()),
        );
    }

    if config.transform.custom_ttl < 0 {
        out.push(
            error(
                "transform.custom_ttl",
                "a custom TTL may not be negative",
                "CFG-033",
            )
            .with_value(config.transform.custom_ttl.to_string()),
        );
    }
}

/// `CFG-037`: naming columns explicitly turns the corresponding automatic mode off.
fn ttl_and_writetime_columns(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    for (kind, automatic, names) in [
        (
            "ttl",
            config.schema.origin.ttl.automatic,
            &config.schema.origin.ttl.names,
        ),
        (
            "writetime",
            config.schema.origin.writetime.automatic,
            &config.schema.origin.writetime.names,
        ),
    ] {
        if automatic && !names.is_empty() {
            out.push(
                notice(
                    &format!("schema.origin.{kind}.automatic"),
                    format!("automatic {kind} resolution is disabled by the explicit column list"),
                    "CFG-037",
                )
                .with_value(names.join(","))
                .with_detail(format!(
                    "`schema.origin.{kind}.names` takes precedence, exactly as Java CDM does"
                )),
            );
        }
    }
}

/// `CFG-038`, the half that needs no schema: each rename must be an `origin:target` pair.
fn column_rename_syntax(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    for pair in &config.schema.origin.column.rename {
        let parts: Vec<&str> = pair.split(':').collect();
        let well_formed = parts.len() == 2 && parts.iter().all(|part| !part.trim().is_empty());
        if !well_formed {
            out.push(
                error(
                    "schema.origin.column.rename",
                    "a column rename must be written `origin:target`",
                    "CFG-038",
                )
                .with_value(pair.clone())
                .with_suggestion("for example `id:new_id`"),
            );
        }
    }
}

/// Extract JSON needs both its origin column and its property mapping, or neither.
fn extract_json(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let feature = &config.feature.extract_json;
    let column = feature
        .origin_column
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());
    let mapping = feature
        .property_mapping
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());

    if column != mapping {
        out.push(
            error(
                "feature.extract_json",
                "extract JSON needs both an origin column and a property mapping",
                "CFG-020",
            )
            .with_detail(if column {
                "`origin_column` is set but `property_mapping` is not"
            } else {
                "`property_mapping` is set but `origin_column` is not"
            }),
        );
    }

    if let Some(mapping) = feature.property_mapping.as_deref() {
        for entry in mapping.split(',').filter(|e| !e.trim().is_empty()) {
            let parts: Vec<&str> = entry.split(':').collect();
            if parts.len() != 2 || parts.iter().any(|part| part.trim().is_empty()) {
                out.push(
                    error(
                        "feature.extract_json.property_mapping",
                        "a property mapping entry must be written `property:column`",
                        "CFG-020",
                    )
                    .with_value(entry.trim().to_owned()),
                );
            }
        }
    }
}

/// `CFG-040`, the half that needs no schema: a writetime filter forces single-row writes.
fn batch_size(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    if config.perfops.batch_size <= 1 {
        return;
    }
    let filtering = config.filter.writetime.min.is_some() || config.filter.writetime.max.is_some();
    if filtering {
        out.push(
            notice(
                "perfops.batch_size",
                "batch size will be coerced to 1",
                "CFG-040",
            )
            .with_value(config.perfops.batch_size.to_string())
            .with_detail(
                "a writetime filter makes each row's inclusion independent, so rows cannot be \
                 batched (MIG-021)",
            ),
        );
    }

    if config.autocorrect.missing_counter {
        out.push(
            warning(
                "autocorrect.missing_counter",
                "re-inserting a missing counter row can double-count it",
                "CFG-040",
            )
            .with_detail(
                "counter updates are not idempotent; a row that was deleted and is re-inserted \
                 accumulates twice",
            ),
        );
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
    use cdm_core::Severity;

    use super::*;

    fn valid() -> CdmConfig {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.src".to_owned());
        config
    }

    fn rules(config: &CdmConfig) -> Vec<String> {
        check(config, ValidationOptions::default())
            .into_iter()
            .filter(Diagnostic::is_blocking)
            .filter_map(|d| d.rule)
            .collect()
    }

    #[test]
    fn cfg_030_constant_column_names_and_values_must_match_in_count() {
        let mut config = valid();
        config.feature.constant_columns.names =
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        config.feature.constant_columns.values = Some("'x',1".to_owned());
        assert_eq!(rules(&config), ["CFG-030"]);

        config.feature.constant_columns.values = Some("'x',1,true".to_owned());
        assert!(rules(&config).is_empty());
    }

    #[test]
    fn cfg_030_a_custom_split_expression_is_what_decides_the_count() {
        let mut config = valid();
        config.feature.constant_columns.names = vec!["a".to_owned(), "b".to_owned()];
        // A comma inside a value is why `splitRegex` exists; with the default separator the
        // three fragments do not match the two names.
        config.feature.constant_columns.values = Some("['x','y'],1".to_owned());
        assert_eq!(rules(&config), ["CFG-030"]);

        // Choosing a separator the values do not contain makes the counts agree again.
        config.feature.constant_columns.values = Some("['x','y']|1".to_owned());
        config.feature.constant_columns.split_regex = r"\|".to_owned();
        assert!(rules(&config).is_empty());
    }

    #[test]
    fn cfg_030_names_without_values_and_values_without_names_are_both_rejected() {
        let mut names_only = valid();
        names_only.feature.constant_columns.names = vec!["a".to_owned()];
        assert_eq!(rules(&names_only), ["CFG-030"]);

        let mut values_only = valid();
        values_only.feature.constant_columns.values = Some("'x'".to_owned());
        assert_eq!(rules(&values_only), ["CFG-030"]);

        // Neither set is the normal case and is silent.
        assert!(rules(&valid()).is_empty());
    }

    #[test]
    fn cfg_031_explode_map_needs_all_three_column_names_or_none() {
        let mut config = valid();
        config.feature.explode_map.origin_column = Some("m".to_owned());
        assert_eq!(rules(&config), ["CFG-031"]);

        config.feature.explode_map.target_key_column = Some("k".to_owned());
        assert_eq!(rules(&config), ["CFG-031"]);

        config.feature.explode_map.target_value_column = Some("v".to_owned());
        assert!(rules(&config).is_empty());

        // A blank is as good as absent, so two blanks and a name is still incomplete.
        config.feature.explode_map.target_value_column = Some("   ".to_owned());
        assert_eq!(rules(&config), ["CFG-031"]);
    }

    #[test]
    fn cfg_032_writetime_bounds_must_be_positive_paired_and_ordered() {
        let mut only_min = valid();
        only_min.filter.writetime.min = Some(100);
        assert_eq!(only_min_rules(&only_min), ["CFG-032"]);

        let mut negative = valid();
        negative.filter.writetime.min = Some(0);
        negative.filter.writetime.max = Some(-1);
        // Both bounds are non-positive, and the maximum is below the minimum: three findings,
        // all reported together rather than the first one only (CFG-021).
        assert_eq!(only_min_rules(&negative), ["CFG-032", "CFG-032", "CFG-032"]);

        let mut inverted = valid();
        inverted.filter.writetime.min = Some(200);
        inverted.filter.writetime.max = Some(100);
        assert_eq!(only_min_rules(&inverted), ["CFG-032"]);

        let mut good = valid();
        good.filter.writetime.min = Some(100);
        good.filter.writetime.max = Some(200);
        assert!(only_min_rules(&good).is_empty());
    }

    /// The writetime rules also trip `CFG-034`, which has its own test; filter it out here.
    fn only_min_rules(config: &CdmConfig) -> Vec<String> {
        rules(config)
            .into_iter()
            .filter(|rule| rule == "CFG-032")
            .collect()
    }

    #[test]
    fn cfg_034_a_writetime_filter_needs_a_resolvable_writetime_column() {
        let mut config = valid();
        config.filter.writetime.min = Some(100);
        config.filter.writetime.max = Some(200);
        config.schema.origin.writetime.automatic = false;
        assert!(rules(&config).contains(&"CFG-034".to_owned()));

        config.schema.origin.writetime.names = vec!["data".to_owned()];
        assert!(!rules(&config).contains(&"CFG-034".to_owned()));

        // With automatic resolution on, only the cluster can answer, so Tier 2 stays quiet.
        config.schema.origin.writetime.automatic = true;
        config.schema.origin.writetime.names.clear();
        assert!(!rules(&config).contains(&"CFG-034".to_owned()));
    }

    #[test]
    fn cfg_033_writetime_and_ttl_transforms_may_not_be_negative() {
        let mut config = valid();
        config.transform.custom_writetime_increment = -1;
        config.transform.custom_writetime = -1;
        config.transform.custom_ttl = -1;
        assert_eq!(rules(&config), ["CFG-033", "CFG-033", "CFG-033"]);

        let mut zero = valid();
        zero.transform.custom_writetime_increment = 0;
        assert!(rules(&zero).is_empty());
    }

    #[test]
    fn cfg_037_naming_columns_explicitly_disables_the_automatic_mode() {
        let mut config = valid();
        config.schema.origin.ttl.names = vec!["data".to_owned()];
        config.schema.origin.writetime.names = vec!["data".to_owned()];

        let diagnostics = check(&config, ValidationOptions::default());
        let notices: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.rule.as_deref() == Some("CFG-037"))
            .collect();
        assert_eq!(notices.len(), 2, "{diagnostics:#?}");
        assert!(notices.iter().all(|d| d.severity == Severity::Info));
        assert!(rules(&config).is_empty(), "a notice is not a failure");
    }

    #[test]
    fn cfg_038_a_rename_must_be_an_origin_colon_target_pair() {
        let mut config = valid();
        config.schema.origin.column.rename = vec![
            "id:new_id".to_owned(),
            "broken".to_owned(),
            "a:b:c".to_owned(),
            ":x".to_owned(),
        ];
        assert_eq!(rules(&config), ["CFG-038", "CFG-038", "CFG-038"]);

        config.schema.origin.column.rename = vec!["id:new_id".to_owned()];
        assert!(rules(&config).is_empty());
    }

    #[test]
    fn cfg_040_a_writetime_filter_coerces_the_batch_size_to_one() {
        let mut config = valid();
        config.perfops.batch_size = 10;
        config.filter.writetime.min = Some(100);
        config.filter.writetime.max = Some(200);

        let diagnostics = check(&config, ValidationOptions::default());
        let coercion = diagnostics
            .iter()
            .find(|d| d.rule.as_deref() == Some("CFG-040"))
            .unwrap();
        assert_eq!(coercion.severity, Severity::Info);
        assert!(coercion.detail.as_deref().unwrap().contains("MIG-021"));

        // A batch size of 1 needs no notice.
        config.perfops.batch_size = 1;
        assert!(!check(&config, ValidationOptions::default())
            .iter()
            .any(|d| d.rule.as_deref() == Some("CFG-040")));
    }

    #[test]
    fn cfg_040_counter_autocorrection_warns_about_double_counting() {
        let mut config = valid();
        config.perfops.batch_size = 5;
        config.autocorrect.missing_counter = true;
        let diagnostics = check(&config, ValidationOptions::default());
        assert!(diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.rule.as_deref() == Some("CFG-040")));
    }

    #[test]
    fn cfg_020_extract_json_needs_both_a_column_and_a_mapping() {
        let mut config = valid();
        config.feature.extract_json.origin_column = Some("doc".to_owned());
        assert_eq!(rules(&config), ["CFG-020"]);

        config.feature.extract_json.property_mapping = Some("name:full_name,age:age".to_owned());
        assert!(rules(&config).is_empty());

        config.feature.extract_json.property_mapping = Some("name:full_name,broken".to_owned());
        assert_eq!(rules(&config), ["CFG-020"]);
    }
}
