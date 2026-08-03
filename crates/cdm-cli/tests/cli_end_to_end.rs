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
        error.message().contains("live schema"),
        "the error must say why: {}",
        error.message()
    );
}

#[test]
fn cli_004_unimplemented_commands_name_the_pull_request() {
    // "not implemented" leaves an evaluator unable to tell a gap from an oversight.
    for (args, pr) in [
        (vec!["cdm", "migrate"], "#21"),
        (vec!["cdm", "serve"], "#42"),
        (vec!["cdm", "mcp"], "#45"),
    ] {
        let error = run_err(&args);
        assert!(
            error.message().contains(pr),
            "{args:?} should name {pr}: {}",
            error.message()
        );
    }
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
