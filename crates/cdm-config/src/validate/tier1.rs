//! Tier 1: syntactic validation (`CFG-020`).
//!
//! Types, ranges, enumeration values and mutually-required groups. Everything here is decidable
//! from the configuration alone, and every check runs even after an earlier one has failed
//! (`CFG-021`).
//!
//! Type and enumeration violations are caught earlier still, by
//! [`ConfigLoader`](crate::ConfigLoader), because only the loader has the raw string the operator
//! wrote; by the time a value reaches `CdmConfig` it is already the right type. What remains here
//! is everything the Rust type system cannot express: ranges, cross-property requirements, and
//! the well-formedness of values whose type is `String` only because CQL, regular expressions and
//! IANA time zones have no dedicated Rust type in the model.

use cdm_core::Diagnostic;

use super::{error, notice, parse_keyspace_table, warning, ValidationOptions};
use crate::model::{CdmConfig, SideConnect};

/// Runs every Tier-1 check.
pub(super) fn check(config: &CdmConfig, _options: ValidationOptions) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    required_properties(config, &mut out);
    connections(config, &mut out);
    ranges(config, &mut out);
    well_formed_values(config, &mut out);
    out
}

/// `CFG-022`: the origin keyspace and table is the only unconditionally required property.
fn required_properties(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    match config.schema.origin.keyspace_table.as_deref() {
        None | Some("") => out.push(
            error(
                "schema.origin.keyspace_table",
                "the origin keyspace and table is required",
                "CFG-022",
            )
            .with_detail("it is the only unconditionally required property")
            .with_suggestion("set `schema.origin.keyspace_table` (spark.cdm.schema.origin.keyspaceTable) to `keyspace.table`"),
        ),
        Some(value) if parse_keyspace_table(value).is_none() => out.push(
            error(
                "schema.origin.keyspace_table",
                "the origin table must be written `keyspace.table`",
                "CFG-022",
            )
            .with_value(value.to_owned())
            .with_suggestion("write it as `my_keyspace.my_table`"),
        ),
        Some(_) => {}
    }

    if let Some(value) = config.schema.target.keyspace_table.as_deref() {
        if value.is_empty() || parse_keyspace_table(value).is_none() {
            out.push(
                error(
                    "schema.target.keyspace_table",
                    "the target table must be written `keyspace.table`",
                    "CFG-023",
                )
                .with_value(value.to_owned())
                .with_suggestion("leave it unset to default to the origin table"),
            );
        }
    }
}

/// `CFG-024`, `CFG-025`, `CFG-026`: the per-side connection rules of `PropertyHelper`.
fn connections(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    for (side, connect) in [
        ("origin", &config.connect.origin),
        ("target", &config.connect.target),
    ] {
        connection(side, connect, out);
    }
}

fn connection(side: &str, connect: &SideConnect, out: &mut Vec<Diagnostic>) {
    let has_host = !connect.host.trim().is_empty();
    let has_scb = connect
        .scb
        .as_ref()
        .is_some_and(|path| !path.as_os_str().is_empty());

    if !has_host && !has_scb {
        out.push(
            error(
                &format!("connect.{side}.host"),
                format!("the {side} connection has neither a host nor a secure-connect-bundle"),
                "CFG-024",
            )
            .with_suggestion(format!("set `connect.{side}.host` or `connect.{side}.scb`")),
        );
        // Java stops looking at this side once it has no connection at all; so do we, since
        // every TLS complaint below would be noise.
        return;
    }

    // CFG-025: with TLS on and no bundle, Java requires all six values.
    if connect.tls.enabled && !has_scb {
        let tls = &connect.tls;
        let missing: Vec<&str> = [
            ("tls.truststore.path", tls.truststore.path.is_none()),
            (
                "tls.truststore.password",
                tls.truststore
                    .password
                    .as_ref()
                    .is_none_or(crate::Secret::is_empty),
            ),
            ("tls.keystore.path", tls.keystore.path.is_none()),
            (
                "tls.keystore.password",
                tls.keystore
                    .password
                    .as_ref()
                    .is_none_or(crate::Secret::is_empty),
            ),
            ("tls.cipher_suites", tls.cipher_suites.is_empty()),
        ]
        .into_iter()
        .filter_map(|(key, absent)| absent.then_some(key))
        .collect();

        for key in missing {
            out.push(
                error(
                    &format!("connect.{side}.{key}"),
                    "TLS is enabled but a required value is not set",
                    "CFG-025",
                )
                .with_suggestion(format!(
                    "set it, or use a secure-connect-bundle via `connect.{side}.scb`"
                )),
            );
        }
        // `truststore.type` cannot be absent: it is an enumeration with a default. Java checks
        // it because its properties are strings; here the type system already guarantees it.
    }

    // CFG-026: empty credentials are unusual but legal — a warning, never an error.
    if connect.username.trim().is_empty() {
        out.push(
            warning(
                &format!("connect.{side}.username"),
                format!("the {side} username is empty"),
                "CFG-026",
            )
            .with_detail("unusual, but valid for a cluster with authentication disabled"),
        );
    }
    if connect.password.is_empty() {
        out.push(
            warning(
                &format!("connect.{side}.password"),
                format!("the {side} password is empty"),
                "CFG-026",
            )
            .with_detail("unusual, but valid for a cluster with authentication disabled"),
        );
    }
}

/// Numeric ranges. `CFG-035` is one of these; the rest guard against values that would make the
/// engine do nothing at all.
fn ranges(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let mut at_least_one = |key: &str, value: u64| {
        if value == 0 {
            out.push(
                error(key, format!("`{key}` must be at least 1"), "CFG-020")
                    .with_value(value.to_string()),
            );
        }
    };
    at_least_one("perfops.num_parts", config.perfops.num_parts);
    at_least_one("perfops.batch_size", u64::from(config.perfops.batch_size));
    at_least_one("perfops.fetch_size", u64::from(config.perfops.fetch_size));
    at_least_one(
        "perfops.connection_pool_size",
        u64::from(config.perfops.connection_pool_size),
    );
    at_least_one(
        "perfops.retry.max_attempts",
        u64::from(config.perfops.retry.max_attempts),
    );
    at_least_one(
        "perfops.max_inflight_reads",
        u64::from(config.perfops.max_inflight_reads),
    );
    at_least_one(
        "perfops.max_inflight_writes",
        u64::from(config.perfops.max_inflight_writes),
    );
    at_least_one(
        "track_run.rerun_multiplier",
        u64::from(config.track_run.rerun_multiplier),
    );
    if let Some(workers) = config.perfops.workers {
        at_least_one("perfops.workers", u64::from(workers));
    }

    if !(1..=100).contains(&config.filter.token_coverage_percent) {
        out.push(
            error(
                "filter.token_coverage_percent",
                "the token coverage percentage must be between 1 and 100",
                "CFG-020",
            )
            .with_value(config.filter.token_coverage_percent.to_string()),
        );
    }

    // CFG-035: a negative guardrail is invalid; zero disables the feature.
    let guardrail = config.feature.guardrail.column_size_kb;
    if guardrail < 0.0 {
        out.push(
            error(
                "feature.guardrail.column_size_kb",
                "the guardrail column size may not be negative",
                "CFG-035",
            )
            .with_value(guardrail.to_string())
            .with_suggestion("use 0 to disable the guardrail"),
        );
    } else if guardrail == 0.0 {
        out.push(
            notice(
                "feature.guardrail.column_size_kb",
                "the column-size guardrail is disabled",
                "CFG-035",
            )
            .with_detail("0 disables the check; set a positive size in KB to enable it"),
        );
    }

    for (key, port) in [
        ("connect.origin.port", config.connect.origin.port),
        ("connect.target.port", config.connect.target.port),
    ] {
        if port == 0 {
            out.push(error(key, "port 0 is not a valid CQL port", "CFG-020"));
        }
    }

    if config.perfops.retry.initial_backoff.get() > config.perfops.retry.max_backoff.get() {
        out.push(
            error(
                "perfops.retry.initial_backoff",
                "the initial backoff exceeds the maximum backoff",
                "CFG-020",
            )
            .with_value(config.perfops.retry.initial_backoff.to_string())
            .with_suggestion("raise `perfops.retry.max_backoff` or lower the initial backoff"),
        );
    }
}

/// Values whose Rust type is `String` but whose grammar is not.
fn well_formed_values(config: &CdmConfig, out: &mut Vec<Diagnostic>) {
    let split_regex = &config.feature.constant_columns.split_regex;
    if let Err(reason) = regex::Regex::new(split_regex) {
        out.push(
            error(
                "feature.constant_columns.split_regex",
                "the constant-column split expression is not a valid regular expression",
                "CFG-020",
            )
            .with_value(split_regex.clone())
            .with_detail(reason.to_string()),
        );
    }

    let zone = &config.transform.codec_timestamp_zone;
    if zone.parse::<chrono_tz::Tz>().is_err() {
        out.push(
            error(
                "transform.codec_timestamp_zone",
                "the timestamp codec time zone is not an IANA zone name",
                "CFG-020",
            )
            .with_value(zone.clone())
            .with_suggestion("use a name such as `UTC`, `America/New_York` or `Europe/London`"),
        );
    }

    if let Some(endpoint) = &config.metrics.otlp.endpoint {
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            out.push(
                error(
                    "metrics.otlp.endpoint",
                    "the OTLP endpoint must be an http or https URL",
                    "CFG-020",
                )
                .with_value(endpoint.clone()),
            );
        }
    }

    // CFG-027 is enforced on the raw value by the loader, which alone can see "the operator
    // wrote an empty list". What survives to here is a list with an empty *element*, which is
    // just as unusable.
    for (key, list) in [
        (
            "schema.origin.column.skip",
            &config.schema.origin.column.skip,
        ),
        (
            "schema.origin.column.rename",
            &config.schema.origin.column.rename,
        ),
        ("schema.origin.ttl.names", &config.schema.origin.ttl.names),
        (
            "schema.origin.writetime.names",
            &config.schema.origin.writetime.names,
        ),
        (
            "feature.constant_columns.names",
            &config.feature.constant_columns.names,
        ),
        ("transform.codecs", &config.transform.codecs),
        (
            "connect.origin.tls.cipher_suites",
            &config.connect.origin.tls.cipher_suites,
        ),
        (
            "connect.target.tls.cipher_suites",
            &config.connect.target.tls.cipher_suites,
        ),
    ] {
        if list.iter().any(|item| item.trim().is_empty()) {
            out.push(
                error(key, format!("`{key}` contains an empty entry"), "CFG-027")
                    .with_value(list.join(","))
                    .with_suggestion("remove the stray separator"),
            );
        }
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
    use crate::secret::Secret;
    use cdm_core::Severity;

    /// A configuration that passes Tier 1, so each test can break exactly one thing.
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
    fn cfg_022_the_origin_keyspace_and_table_is_the_only_required_property() {
        // Nothing at all set: exactly one blocking complaint, and it is CFG-022.
        let empty = CdmConfig::default();
        assert_eq!(rules(&empty), ["CFG-022"]);

        // Supplying it is enough to make a default configuration valid.
        assert!(rules(&valid()).is_empty());

        let mut malformed = valid();
        malformed.schema.origin.keyspace_table = Some("no_dot".to_owned());
        assert_eq!(rules(&malformed), ["CFG-022"]);

        let mut blank = valid();
        blank.schema.origin.keyspace_table = Some(String::new());
        assert_eq!(rules(&blank), ["CFG-022"]);
    }

    #[test]
    fn cfg_023_a_malformed_target_table_is_rejected_but_an_absent_one_is_not() {
        let mut config = valid();
        config.schema.target.keyspace_table = None;
        assert!(rules(&config).is_empty());

        config.schema.target.keyspace_table = Some("nodot".to_owned());
        assert_eq!(rules(&config), ["CFG-023"]);
    }

    #[test]
    fn cfg_024_a_side_needs_either_a_host_or_a_bundle() {
        let mut config = valid();
        config.connect.origin.host = String::new();
        assert_eq!(rules(&config), ["CFG-024"]);

        // A bundle is the other way to satisfy it.
        config.connect.origin.scb = Some("/tmp/scb.zip".into());
        assert!(rules(&config).is_empty());

        // Both sides are checked, and both are reported at once.
        let mut both = valid();
        both.connect.origin.host = "  ".to_owned();
        both.connect.target.host = String::new();
        assert_eq!(rules(&both), ["CFG-024", "CFG-024"]);
    }

    #[test]
    fn cfg_025_enabling_tls_without_a_bundle_requires_the_whole_store_configuration() {
        let mut config = valid();
        config.connect.origin.tls.enabled = true;
        // Every missing value is named, not just the first.
        let reported = rules(&config);
        assert_eq!(reported, ["CFG-025"; 4], "{reported:?}");

        config.connect.origin.tls.truststore.path = Some("/t.jks".into());
        config.connect.origin.tls.truststore.password = Some(Secret::new("x"));
        config.connect.origin.tls.keystore.path = Some("/k.jks".into());
        config.connect.origin.tls.keystore.password = Some(Secret::new("y"));
        assert!(rules(&config).is_empty());

        // An empty cipher-suite list is as bad as an absent store.
        config.connect.origin.tls.cipher_suites.clear();
        assert_eq!(rules(&config), ["CFG-025"]);
    }

    #[test]
    fn cfg_025_a_bundle_exempts_a_side_from_the_tls_store_requirements() {
        let mut config = valid();
        config.connect.target.tls.enabled = true;
        config.connect.target.scb = Some("/tmp/scb.zip".into());
        assert!(rules(&config).is_empty());
    }

    #[test]
    fn cfg_026_empty_credentials_warn_rather_than_fail() {
        let mut config = valid();
        config.connect.origin.username = String::new();
        config.connect.target.password = Secret::new("");

        let diagnostics = check(&config, ValidationOptions::default());
        let warnings: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(warnings.len(), 2, "{diagnostics:#?}");
        assert!(warnings
            .iter()
            .all(|d| d.rule.as_deref() == Some("CFG-026")));
        assert!(rules(&config).is_empty(), "a warning is not a failure");
    }

    #[test]
    fn cfg_027_a_list_with_an_empty_entry_is_rejected() {
        let mut config = valid();
        config.schema.origin.column.skip = vec!["a".to_owned(), String::new()];
        assert_eq!(rules(&config), ["CFG-027"]);
    }

    #[test]
    fn cfg_035_a_negative_guardrail_is_invalid_and_zero_merely_disables_it() {
        let mut config = valid();
        config.feature.guardrail.column_size_kb = -0.5;
        assert_eq!(rules(&config), ["CFG-035"]);

        config.feature.guardrail.column_size_kb = 0.0;
        assert!(rules(&config).is_empty());
        let disabled = check(&config, ValidationOptions::default());
        assert!(disabled
            .iter()
            .any(|d| d.severity == Severity::Info && d.rule.as_deref() == Some("CFG-035")));

        config.feature.guardrail.column_size_kb = 10.0;
        assert!(check(&config, ValidationOptions::default()).is_empty());
    }

    #[test]
    fn cfg_020_numeric_ranges_that_would_stall_the_engine_are_rejected() {
        let mut config = valid();
        config.perfops.num_parts = 0;
        config.perfops.batch_size = 0;
        config.perfops.fetch_size = 0;
        config.perfops.workers = Some(0);
        config.track_run.rerun_multiplier = 0;
        config.filter.token_coverage_percent = 0;
        config.connect.origin.port = 0;
        assert_eq!(rules(&config).len(), 7, "{:#?}", rules(&config));

        let mut coverage = valid();
        coverage.filter.token_coverage_percent = 101;
        assert_eq!(rules(&coverage), ["CFG-020"]);
    }

    #[test]
    fn cfg_020_a_backoff_ceiling_below_the_floor_is_rejected() {
        let mut config = valid();
        config.perfops.retry.initial_backoff = crate::types::DurationSetting::from_secs(30);
        config.perfops.retry.max_backoff = crate::types::DurationSetting::from_secs(1);
        assert_eq!(rules(&config), ["CFG-020"]);
    }

    #[test]
    fn cfg_020_string_typed_values_with_a_grammar_are_checked() {
        let mut config = valid();
        config.feature.constant_columns.split_regex = "([".to_owned();
        config.transform.codec_timestamp_zone = "Mars/Olympus".to_owned();
        config.metrics.otlp.endpoint = Some("localhost:4317".to_owned());
        assert_eq!(rules(&config).len(), 3);

        let mut good = valid();
        good.transform.codec_timestamp_zone = "America/New_York".to_owned();
        good.metrics.otlp.endpoint = Some("https://otel:4317".to_owned());
        good.feature.constant_columns.split_regex = r"\s*,\s*".to_owned();
        assert!(rules(&good).is_empty());
    }
}
