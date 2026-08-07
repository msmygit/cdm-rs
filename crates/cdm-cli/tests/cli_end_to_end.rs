//! End-to-end tests driving the CLI the way an operator does (`CLI-001`..`CLI-005`).
//!
//! These call `cdm_cli::run` with a buffer rather than spawning a process, so they assert on the
//! bytes a user would see without paying for a binary launch per case.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cdm_cli::cli::Cli;
use cdm_cli::exit::Exit;
use clap::Parser;

/// Runs `cdm …` and returns the exit code plus whatever was written to stdout.
fn run(args: &[&str]) -> (Exit, String) {
    let cli = Cli::try_parse_from(args).expect("arguments must parse");
    let mut out = Vec::new();
    let exit = cdm_cli::run(&cli, &mut out).expect("command must not error");
    (exit, String::from_utf8(out).expect("output is UTF-8"))
}

/// Runs `cdm …` expecting the command itself to fail.
fn run_err(args: &[&str]) -> cdm_core::CdmError {
    let cli = Cli::try_parse_from(args).expect("arguments must parse");
    let mut out = Vec::new();
    cdm_cli::run(&cli, &mut out).expect_err("command was expected to fail")
}

#[test]
fn cli_001_version_reports_the_driver() {
    let (exit, text) = run(&["cdm", "version"]);
    assert_eq!(exit, Exit::Success);
    // The driver determines which clusters cdm-rs can talk to, so it belongs in `version`.
    assert!(text.contains("scylla-rust-driver"), "{text}");
}

#[test]
fn cli_005_json_output_is_a_single_parseable_document() {
    let (exit, text) = run(&["cdm", "--output", "json", "version"]);
    assert_eq!(exit, Exit::Success);

    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(value["driver"], "scylla-rust-driver");
}

#[test]
fn cli_007_completions_are_generated_for_every_supported_shell() {
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let (exit, script) = run(&["cdm", "completions", shell]);
        assert_eq!(exit, Exit::Success);
        assert!(
            script.contains("cdm"),
            "{shell} completion should mention the binary"
        );
        assert!(!script.is_empty(), "{shell} completion was empty");
    }
}

#[test]
fn cfg_003_the_schema_command_emits_the_json_schema() {
    let (exit, text) = run(&["cdm", "config", "schema"]);
    assert_eq!(exit, Exit::Success);

    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON Schema");
    assert!(
        value["$defs"].is_object() || value["properties"].is_object(),
        "expected a schema document, got: {text:.200}"
    );
}

#[test]
fn cfg_022_a_missing_origin_table_is_reported_not_hidden() {
    // Nothing configured at all: the only required property is absent.
    let (exit, text) = run(&["cdm", "config", "validate"]);

    assert_eq!(
        exit,
        Exit::Completed,
        "an invalid configuration is a finding, not a crash"
    );
    assert!(text.contains("Invalid"), "{text}");
    assert!(
        text.contains("schema.origin.keyspace_table") || text.contains("origin keyspace"),
        "the report must name the missing property:\n{text}"
    );
}

#[test]
fn cfg_021_validation_reports_every_violation_at_once() {
    let (_, text) = run(&[
        "cdm",
        "config",
        "validate",
        "--set",
        "feature.constant_columns.names=a,b",
        "--set",
        "feature.constant_columns.values=1",
    ]);

    // Both the missing table and the constant-column mismatch must appear; stopping at the first
    // is what CFG-021 forbids.
    assert!(text.contains("Invalid"), "{text}");
    let errors = text.matches("rule:").count();
    assert!(errors >= 2, "expected several findings, got:\n{text}");
}

#[test]
fn cli_005_validation_findings_are_machine_readable() {
    let (_, text) = run(&["cdm", "--output", "json", "config", "validate"]);
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");

    assert_eq!(value["valid"], false);
    assert!(
        value["diagnostics"]
            .as_array()
            .is_some_and(|d| !d.is_empty()),
        "diagnostics must survive into JSON: {text}"
    );
}

#[test]
fn cfg_020_tier_three_refuses_rather_than_silently_running_fewer_checks() {
    // Quietly downgrading to tier 2 is how a configuration reaches production unvalidated.
    let error = run_err(&["cdm", "config", "validate", "--tier", "schema"]);
    assert!(
        error.message().contains("live cluster session"),
        "the error must say why: {}",
        error.message()
    );
}

#[test]
fn cli_004_unimplemented_commands_name_what_is_missing() {
    // "not implemented" leaves an evaluator unable to tell a gap from an oversight. A pull-request
    // number was the first attempt and it aged badly: it makes the reader go and look the number
    // up, and it becomes a lie the moment the roadmap is renumbered. Naming the missing crate
    // answers the question in the message.
    for (args, crate_name) in [
        (vec!["cdm", "serve"], "cdm-api"),
        (vec!["cdm", "mcp"], "cdm-mcp"),
        (vec!["cdm", "cluster"], "cdm-cluster"),
    ] {
        let error = run_err(&args);
        let message = error.message();
        assert!(
            message.contains(crate_name),
            "{args:?} should say that {crate_name} is what is missing: {message}"
        );
        assert!(
            !message.contains("PR #"),
            "a roadmap number is not an explanation: {message}"
        );
    }
}

#[test]
fn cdc_031_the_codec_catalogue_needs_no_cluster() {
    // The question "will cdm-rs convert my `text` column into a `timestamp`?" is asked *before*
    // there is a configuration, let alone a cluster. A command that demanded either would not be
    // usable at the moment it is needed.
    let (exit, text) = run(&["cdm", "codecs"]);
    assert_eq!(exit, Exit::Success);
    assert!(text.contains("TIMESTAMP_STRING_MILLIS"), "{text}");
    assert!(text.contains("->"), "the type pairs are the point: {text}");
}

#[test]
fn cli_005_the_codec_catalogue_is_machine_readable() {
    let (exit, text) = run(&["cdm", "--output", "json", "codecs"]);
    assert_eq!(exit, Exit::Success);

    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let conversions = value["conversions"].as_array().expect("an array");
    assert!(!conversions.is_empty());
    for conversion in conversions {
        assert!(conversion["from"].is_string(), "{conversion}");
        assert!(conversion["to"].is_string(), "{conversion}");
    }
}

#[test]
fn trk_034_runs_resume_says_which_half_of_it_is_missing() {
    // `cdm-track` computes the resume work list already; what is absent is the harness's ability
    // to be handed one. A message that said only "not implemented" would hide the fact that
    // `cdm runs show` can already answer what is outstanding.
    let error = run_err(&["cdm", "runs", "resume"]);
    let message = error.message();
    assert!(message.contains("cdm runs show"), "{message}");
}

#[test]
fn val_015_sample_is_rejected_before_a_session_is_opened() {
    // `--sample 0` would plan a run that reads nothing and then report everything it did not look
    // at as fine. It is Tier-1's rule for `filter.token_coverage_percent`, and the flag must not
    // be able to smuggle past it.
    let error = run_err(&[
        "cdm",
        "validate",
        "--sample",
        "0",
        "--set",
        "schema.origin.keyspace_table=ks.tbl",
    ]);
    let message = error.message();
    assert!(
        message.contains("between 1 and 100"),
        "the diagnostic must state the range: {message}"
    );
}

#[test]
fn val_015_the_two_validate_flags_are_refused_on_the_other_jobs() {
    // A `cdm migrate --sample 5` that parsed would be a migration that silently moved a twentieth
    // of the data and reported success. clap refusing the flag is a better answer than a runtime
    // error nobody reads, and `VAL-015` gives both flags to validate alone.
    for args in [
        vec!["cdm", "migrate", "--sample", "5"],
        vec!["cdm", "migrate", "--keys-only"],
        vec!["cdm", "guardrail", "--keys-only"],
        vec!["cdm", "plan", "--sample", "5"],
    ] {
        assert!(
            Cli::try_parse_from(&args).is_err(),
            "{args:?} must not parse"
        );
    }
}

#[test]
fn cli_001_a_job_command_validates_before_it_connects() {
    // `cdm migrate` used to answer "not yet" while the migrate engine was finished. It now runs,
    // which means the first thing it can fail on is the configuration -- and it must fail there
    // rather than opening a session and discovering the same thing after a TLS handshake.
    let error = run_err(&["cdm", "migrate"]);
    let message = error.message();

    assert!(
        message.contains("schema.origin.keyspace_table"),
        "the diagnostic must name the property to fix: {message}"
    );
    assert!(
        !message.contains("not yet"),
        "migrate is implemented; saying otherwise sends an evaluator away: {message}"
    );
}

#[test]
fn cfg_021_a_job_command_reports_every_blocking_problem_at_once() {
    // One round trip per mistake is the complaint CFG-021 exists to answer, and a job command is
    // where an operator meets it.
    let error = run_err(&[
        "cdm",
        "validate",
        "--set",
        "perfops.consistency.read=NOT_A_LEVEL",
    ]);
    let message = error.message();
    assert!(
        message.contains("problem(s)"),
        "the summary must count them: {message}"
    );
}

#[test]
fn cli_002_a_java_properties_file_loads_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cdm.properties");
    std::fs::write(
        &path,
        "spark.cdm.schema.origin.keyspaceTable=ks.tbl\n\
         spark.cdm.connect.origin.host=origin.example.com\n\
         spark.cdm.perfops.numParts=1234\n",
    )
    .unwrap();

    let (exit, text) = run(&[
        "cdm",
        "config",
        "validate",
        "--properties-file",
        path.to_str().unwrap(),
    ]);

    assert_eq!(
        exit,
        Exit::Success,
        "a valid Java configuration must validate cleanly:\n{text}"
    );
}

#[test]
fn cli_003_conversion_produces_canonical_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cdm.properties");
    std::fs::write(
        &path,
        "spark.cdm.schema.origin.keyspaceTable=ks.tbl\n\
         spark.cdm.perfops.numParts=1234\n",
    )
    .unwrap();

    let (exit, text) = run(&["cdm", "config", "convert", "--from", path.to_str().unwrap()]);

    assert_eq!(exit, Exit::Success);
    assert!(
        text.contains("num_parts"),
        "canonical names expected:\n{text}"
    );
    assert!(text.contains("1234"), "the value must survive:\n{text}");
}

#[test]
fn sec_001_explain_never_prints_a_secret() {
    let (_, text) = run(&[
        "cdm",
        "config",
        "explain",
        "connect.origin.password",
        "--set",
        "connect.origin.password=hunter2",
    ]);

    assert!(
        !text.contains("hunter2"),
        "a password must never reach the terminal or the shell history:\n{text}"
    );
}
