//! The Java SIT parity suite, against a real node (`TST-003`).
//!
//! Nineteen cases, one test each, ported from `SIT/{smoke,features,regression}` in the Java tree.
//! The declarative half — what a case *is* — lives in [`cdm_testkit::sit`] and is unit-tested
//! there, on every platform, with no container. This file is the half that needs a node: it starts
//! one, runs each case's steps against it, and asserts the counter block and the target's final
//! state.
//!
//! | Java case | Test | Covers |
//! |---|---|---|
//! | `smoke/00_test_harness` | [`tst_003_the_harness_can_load_a_schema_and_read_it_back`] | the fixture itself |
//! | `smoke/01_basic_kvp` | [`tst_003_two_rows_migrate_and_then_validate_clean`] | `MIG-001`, `VAL-001` |
//! | `smoke/02_autocorrect_kvp` | [`tst_003_autocorrect_repairs_a_missing_row_and_a_mismatched_one`] | `VAL-003`, `VAL-007` |
//! | `smoke/03_ttl_writetime` | [`tst_003_ttl_and_writetime_are_carried_across_as_the_per_row_maximum`] | `FEA-040`..`FEA-046` |
//! | `smoke/04_counters` | [`tst_003_a_counter_delta_lands_once_and_a_deleted_counter_needs_the_explicit_opt_in`] | `MIG-030`..`MIG-032`, `VAL-004` |
//! | `smoke/05_reserved_keyword` | [`tst_003_a_column_named_after_a_reserved_word_is_quoted_everywhere`] | `SCH-002` |
//! | `smoke/06_vector` | [`tst_003_a_float_vector_column_round_trips`] | `CDC-004` |
//! | `features/01_constant_column` | [`tst_003_constant_columns_supply_target_columns_the_origin_does_not_have`] | `FEA-010`..`FEA-013` |
//! | `features/02_explode_map` | [`tst_003_a_map_column_explodes_into_one_target_row_per_entry`] | `FEA-020`..`FEA-023` |
//! | `features/03_codec` | [`tst_003_string_columns_convert_into_typed_target_columns`] | `CDC-020`..`CDC-022` |
//! | `features/04_udt_mapper` | [`tst_003_udts_nested_in_collections_are_converted_field_by_field`] | `CDC-013`, `CDC-014` |
//! | `features/05_guardrail` | [`tst_003_the_guardrail_finds_three_oversized_rows_and_exits_non_zero`] | `GRD-001`..`GRD-003` |
//! | `features/06_constant_column_remove` | [`tst_003_origin_columns_the_target_does_not_have_are_dropped`] | `FEA-014` |
//! | `features/07_constant_column_replace` | [`tst_003_constant_columns_replace_the_origins_own_key_columns`] | `FEA-014` |
//! | `features/08_map_columns_origin_target` | [`tst_003_columns_map_by_name_across_two_differently_shaped_tables`] | `SCH-003`, `SCH-004` |
//! | `regression/01_explode_map_with_constants` | [`tst_003_an_exploded_map_a_constant_column_and_a_codec_compose`] | `FEA-010`+`FEA-020`+`CDC-020` |
//! | `regression/02_ColumnRenameWithConstantsAndExplode` | [`tst_003_quoted_hyphenated_identifiers_survive_a_rename_and_an_explode`] | `SCH-002`, `SCH-003` |
//! | `regression/03_performance` | [`tst_003_four_thousand_rows_across_thirty_two_ranges_lose_nothing`] | `ENG-*`, `MIG-020` |
//! | `regression/04_null_ts_in_pk` | [`tst_003_a_null_in_a_target_key_column_is_substituted`] | `MIG-013` |
//!
//! # How to run it
//!
//! ```text
//! cargo xtask sit
//! ```
//!
//! which is `cargo build -p cdm-cli --bin cdm` followed by
//! `cargo test -p cdm-testkit --test sit_it -- --ignored --test-threads=1`. The thread count is
//! not advice: every case shares one node and one pair of keyspaces, and two cases that both own
//! `origin.*` cannot run at once.
//!
//! Per `TST-102` these skip — they do not fail — when no container runtime is available.
//!
//! # Four cases cannot run yet, and it is not the suite's fault
//!
//! Each carries a `blocked <reason>` line in its `case.txt`, and the runner prints it as
//! `BLOCKED <case>: <reason>` and returns rather than asserting. `#[ignore]` could not carry this
//! distinction: `--ignored` runs *only* ignored tests, and every case here is ignored because
//! every case needs a container, so the attribute says nothing about whether a case can pass.
//!
//! Three of the four are the same gap: validate issues one target lookup per *record* where an
//! explode map produces one target row per map *entry*, so `ComparisonPlan` marks the exploded
//! columns unobtainable and every entry reports missing. The fourth is `VAL-018`, the TTL and
//! writetime an autocorrected row must carry. The cases are written out in full and are expected
//! to pass unchanged when that work lands; nothing in `tests/sit/` needs to change.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::process::Command;
use std::sync::OnceLock;

use cdm_testkit::sit::{SitCase, SitStep};
use cdm_testkit::{
    compare_counter_block, compare_cqlsh, render_properties, skip_without_container_runtime,
    ClusterFixture, Engine,
};

/// The engine the suite runs against.
///
/// Cassandra 5.0 rather than the 4.1 the other `_it` suites default to, because `smoke/06_vector`
/// needs `vector<float, 3>` and a suite that silently skipped its only Cassandra-5 case would be
/// claiming parity it had not tested (`CDC-004`). `CDM_IT_ENGINES` still overrides it, and the
/// vector case skips rather than fails on an engine without vectors.
fn engine() -> Engine {
    cdm_testkit::engines_under_test()
        .expect("CDM_IT_ENGINES names an unknown engine")
        .into_iter()
        .next()
        .unwrap_or_else(|| Engine::cassandra("5.0"))
}

/// The one node every case shares.
///
/// Java's harness starts one container for the whole suite and resets the keyspaces between
/// cases; this does the same, for the same reason. Nineteen containers would take longer to start
/// than the suite takes to run, and the CQL port is a singleton on the host either way.
static NODE: OnceLock<Shared> = OnceLock::new();

/// The container name the suite's node is given. `cargo xtask sit` knows it too.
const SIT_CONTAINER: &str = "cdm-sit-node";

struct Shared {
    runtime: tokio::runtime::Runtime,
    fixture: ClusterFixture,
}

fn node() -> &'static Shared {
    NODE.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a tokio runtime");
        // A named container, because this one is held in a `static` and a `static` is never
        // dropped: nothing stops it when the process exits, and the next run would then fail to
        // bind the fixed CQL port. `cargo xtask sit` removes this exact name before and after.
        let options = cdm_testkit::FixtureOptions::default()
            .with_container_name(Some(SIT_CONTAINER.to_owned()));
        let fixture = runtime
            .block_on(ClusterFixture::start_with(&engine(), &options))
            .expect("the SIT node must start");
        // Java's `environment.sh` creates these once, before any case runs.
        runtime
            .block_on(fixture.exec_cql(
                "CREATE KEYSPACE IF NOT EXISTS origin WITH replication = \
                 {'class':'SimpleStrategy','replication_factor':1}; \
                 CREATE KEYSPACE IF NOT EXISTS target WITH replication = \
                 {'class':'SimpleStrategy','replication_factor':1};",
            ))
            .expect("the origin and target keyspaces must be creatable");
        Shared { runtime, fixture }
    })
}

/// The `cdm` binary, found next to this test binary.
///
/// `CARGO_BIN_EXE_*` only names binaries of the *same* package, and this crate deliberately has no
/// dependency on `cdm-cli` — the suite drives the binary as a black box, exactly as Java's SIT
/// drives `spark-submit` rather than `CopyJobSession`. `cargo xtask sit` builds it first.
fn cdm_binary() -> std::path::PathBuf {
    let mut dir = std::env::current_exe().expect("the test binary has a path");
    dir.pop(); // deps/
    if dir.ends_with("deps") {
        dir.pop();
    }
    let binary = dir.join(if cfg!(windows) { "cdm.exe" } else { "cdm" });
    assert!(
        binary.is_file(),
        "the `cdm` binary is not at {}; run `cargo xtask sit`, which builds it first",
        binary.display()
    );
    binary
}

/// Runs every step of `case`, panicking with the first failure.
fn run(case: &SitCase) {
    if let Some(reason) = case.blocked() {
        // Reported rather than asserted, and loudly: a case that cannot run is a fact about
        // cdm-rs, not about the case, and burying it in a green test result is how a parity suite
        // comes to certify parity it never measured.
        eprintln!(
            "BLOCKED {}: {reason}\n         the case is complete; it is expected to pass \
             unchanged once the wiring lands.",
            case.id()
        );
        return;
    }
    let shared = node();
    let contact = shared.fixture.contact_point();
    let (host, port) = contact
        .rsplit_once(':')
        .expect("a contact point is host:port");
    let port: u16 = port.parse().expect("a numeric port");

    let work = tempfile::tempdir().expect("a working directory");

    for (index, step) in case.steps().iter().enumerate() {
        let where_ = format!("{} step {}", case.id(), index + 1);
        match step {
            SitStep::Cql { file } => {
                let script = case.read(file).expect("the script is readable");
                shared
                    .runtime
                    .block_on(shared.fixture.exec_cql(&script))
                    .unwrap_or_else(|e| panic!("{where_}: {file}: {e}"));
            }
            SitStep::Job {
                job,
                properties,
                expect,
            } => {
                let template = case.read(properties).expect("the properties are readable");
                let path = work.path().join(properties);
                std::fs::write(&path, render_properties(&template, host, port))
                    .expect("the rendered properties are writable");

                // Run from the scratch directory: `VAL-012`'s diff log defaults to
                // `cdm_logs/cdm_diff.log` *relative to the working directory*, and a suite that
                // left one in the package root would have every run of `cargo test` create a file
                // in the source tree.
                let output = Command::new(cdm_binary())
                    .current_dir(work.path())
                    .arg(job.as_str())
                    .arg("--properties-file")
                    .arg(&path)
                    .output()
                    .unwrap_or_else(|e| panic!("{where_}: cannot run `cdm {}`: {e}", job.as_str()));
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // `CLI-004`: `0` is a clean run and `1` is a run that completed and found
                // discrepancies — `smoke/04_counters`' `fix` step leaves one counter row
                // uncorrected on purpose, and Java's guardrail exits `0` whatever it found where
                // cdm-rs exits `1` (divergence 33). Both mean the job ran, which is the
                // precondition for the counter block below being worth reading. `2`..`5` are a
                // configuration error, a connection error, an interruption or an internal error:
                // the run never happened, and an empty block would say so much less clearly.
                assert!(
                    matches!(output.status.code(), Some(0 | 1)),
                    "{where_}: `cdm {}` exited {:?}, which CLI-004 reserves for a run that never \
                     completed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
                    job.as_str(),
                    output.status.code()
                );

                let expected = case.read(expect).expect("the expectation is readable");
                if let Err(problem) = compare_counter_block(&expected, &stdout) {
                    panic!("{where_} ({expect}): {problem}\n--- stderr ---\n{stderr}");
                }
            }
            SitStep::Check { query, expected } => {
                let script = case.read(query).expect("the query is readable");
                let actual = shared
                    .runtime
                    .block_on(shared.fixture.exec_cql(&script))
                    .unwrap_or_else(|e| panic!("{where_}: {query}: {e}"));
                let want = case.read(expected).expect("the expectation is readable");
                if let Err(problem) = compare_cqlsh(&want, &actual) {
                    panic!("{where_} ({expected}): {problem}");
                }
            }
        }
    }
}

/// Loads and runs one case by phase and name.
fn case(phase: &str, name: &str) {
    let case = cdm_testkit::sit::case(phase, name)
        .unwrap_or_else(|e| panic!("tests/sit/{phase}/{name} does not load: {e}"));
    run(&case);
}

// ---------------------------------------------------------------------------------------- smoke

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_the_harness_can_load_a_schema_and_read_it_back() {
    skip_without_container_runtime!();
    case("smoke", "00_test_harness");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_two_rows_migrate_and_then_validate_clean() {
    skip_without_container_runtime!();
    case("smoke", "01_basic_kvp");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_autocorrect_repairs_a_missing_row_and_a_mismatched_one() {
    skip_without_container_runtime!();
    case("smoke", "02_autocorrect_kvp");
}

/// Blocked: `crates/cdm-cli/src/harness/build.rs::migrate` builds the job with
/// `MigrateFeatures::default()`, so `schema.origin.column.ttl.names` and
/// `schema.origin.column.writetime.names` are parsed, validated and then never used. The target
/// rows arrive with the write's own timestamp and no TTL. `FEA-040`..`FEA-046` are implemented in
/// `cdm-feature` and covered by that crate's own tests; what is missing is four lines of wiring.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_ttl_and_writetime_are_carried_across_as_the_per_row_maximum() {
    skip_without_container_runtime!();
    case("smoke", "03_ttl_writetime");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_a_counter_delta_lands_once_and_a_deleted_counter_needs_the_explicit_opt_in() {
    skip_without_container_runtime!();
    case("smoke", "04_counters");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_a_column_named_after_a_reserved_word_is_quoted_everywhere() {
    skip_without_container_runtime!();
    case("smoke", "05_reserved_keyword");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_a_float_vector_column_round_trips() {
    skip_without_container_runtime!();
    let fixture = &node().fixture;
    if !fixture.supports_vectors() {
        // CDC-004: a capability only one engine implements is skipped, never failed.
        eprintln!(
            "skipping: {} does not implement vector<t, n> (CDC-004)",
            fixture.engine()
        );
        return;
    }
    case("smoke", "06_vector");
}

// ------------------------------------------------------------------------------------- features

/// Blocked: `build.rs` resolves the column mapping with `MappingOptions::default()`, whose
/// `constants` field is empty, so `feature.constantColumns.*` never reaches `ColumnMapping`. The
/// run fails at startup with `SCH-006`: "target primary-key column `const1` … has no source".
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_constant_columns_supply_target_columns_the_origin_does_not_have() {
    skip_without_container_runtime!();
    case("features", "01_constant_column");
}

/// Blocked: as above — `MappingOptions::default()` leaves `explode_map` at `None`, so
/// `feature.explodeMap.*` is ignored and the run fails at startup with `SCH-006` on the target's
/// `fruit` key column.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_a_map_column_explodes_into_one_target_row_per_entry() {
    skip_without_container_runtime!();
    case("features", "02_explode_map");
}

/// Blocked: `build.rs::codec_planner` calls `CodecRegistry::with_builtins(&enabled, None)`. The
/// codec *names* are read from the configuration, but the format options are not, so
/// `TIMESTAMP_STRING_FORMAT` is rejected at startup with "requires
/// transform.codecs.timestamp_format and transform.codecs.timestamp_format_zone" even when both
/// are set. `CDC-020`..`CDC-022` are implemented; the `Some(_)` argument is missing.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_string_columns_convert_into_typed_target_columns() {
    skip_without_container_runtime!();
    case("features", "03_codec");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_udts_nested_in_collections_are_converted_field_by_field() {
    skip_without_container_runtime!();
    case("features", "04_udt_mapper");
}

/// Blocked: `build.rs::job` returns a configuration error for `JobKind::Guardrail`, saying the job
/// "needs a paged origin reader, which lands with the guardrail row source". The job itself is
/// implemented and covered by `cdm-engine`'s `guardrail_it`; what is missing is the reader.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_the_guardrail_finds_three_oversized_rows_and_exits_non_zero() {
    skip_without_container_runtime!();
    case("features", "05_guardrail");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_origin_columns_the_target_does_not_have_are_dropped() {
    skip_without_container_runtime!();
    case("features", "06_constant_column_remove");
}

/// Blocked: `MappingOptions::default()`, as `features/01`.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_constant_columns_replace_the_origins_own_key_columns() {
    skip_without_container_runtime!();
    case("features", "07_constant_column_replace");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_columns_map_by_name_across_two_differently_shaped_tables() {
    skip_without_container_runtime!();
    case("features", "08_map_columns_origin_target");
}

// ----------------------------------------------------------------------------------- regression

/// Blocked: `MappingOptions::default()` — this case needs both constant columns and the explode
/// map, and a codec besides.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_an_exploded_map_a_constant_column_and_a_codec_compose() {
    skip_without_container_runtime!();
    case("regression", "01_explode_map_with_constants");
}

/// Blocked: `MappingOptions::default()` — this case additionally needs `rename`, which is the same
/// field of the same defaulted struct.
#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_quoted_hyphenated_identifiers_survive_a_rename_and_an_explode() {
    skip_without_container_runtime!();
    case("regression", "02_ColumnRenameWithConstantsAndExplode");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_four_thousand_rows_across_thirty_two_ranges_lose_nothing() {
    skip_without_container_runtime!();
    case("regression", "03_performance");
}

#[test]
#[ignore = "needs a container runtime; run with `cargo xtask sit`"]
fn tst_003_a_null_in_a_target_key_column_is_substituted() {
    skip_without_container_runtime!();
    case("regression", "04_null_ts_in_pk");
}
