//! The tier-2 macro-benchmark, against real nodes (`TST-060`, `NFR-004`).
//!
//! The unit tests in `cdm_testkit::macrobench` prove the *arithmetic*: that the dataset is a pure
//! function of the seed, that throughput is rows over seconds, that the bencher line says
//! nanoseconds per row. They cannot prove the thing the tier exists for — that a full migration
//! through the real scheduler, the real executor and the real migrate job moves every seeded row
//! between two real clusters, and how fast. That is a fact about containers.
//!
//! | Claim | Test |
//! |---|---|
//! | A migration runs end to end and reports a throughput (`TST-060`, `NFR-004`) | [`nfr_004_a_macro_benchmark_measures_a_real_migration`] |
//! | Without a container runtime the benchmark skips and succeeds (`TST-102`) | [`tst_102_the_macro_benchmark_skips_without_a_container_runtime`] |
//!
//! Run it with
//! `cargo test -p cdm-testkit --features macrobench --test macrobench -- --ignored --nocapture`,
//! or through `cargo xtask bench`.
//!
//! # Why the row count here is not the default
//!
//! [`MacroBenchSpec::default`] asks for 100,000 rows, which is the figure worth *recording*. This
//! suite asks for far fewer, because it is a correctness test of the harness that happens to
//! print a throughput: it must stay runnable inside an ordinary integration run. The number to
//! quote in `docs/BENCHMARKS.md` comes from `cargo xtask bench`, at the default spec.

// The whole file needs the feature that brings the harness in; without it there is nothing here.
#![cfg(feature = "macrobench")]
// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on the driver's own `SessionBuilder::build()`, reached through the
// harness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use cdm_testkit::macrobench::{run_macro_bench_or_skip, MacroBenchSpec, BENCHER_NAME};
use cdm_testkit::ContainerRuntime;

/// The environment variable that widens this suite to a spec worth recording.
///
/// `CDM_BENCH_ROWS=100000` reproduces the default spec, and therefore the figure quoted in
/// `docs/BENCHMARKS.md`, without editing this file — which is the difference between a recorded
/// number somebody can check and a recorded number somebody has to trust.
const ROWS_ENV: &str = "CDM_BENCH_ROWS";

/// The spec this suite measures: wide enough that the per-row cost dominates the fixed cost of
/// starting the scheduler, small enough to seed in seconds.
fn spec() -> MacroBenchSpec {
    let rows = std::env::var(ROWS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|rows| *rows > 0)
        .unwrap_or(20_000);
    MacroBenchSpec {
        rows,
        columns: 16,
        ..MacroBenchSpec::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a container runtime; run with --ignored or via `cargo xtask bench`"]
async fn nfr_004_a_macro_benchmark_measures_a_real_migration() {
    let spec = spec();
    let Some(result) = run_macro_bench_or_skip(&spec).await.unwrap() else {
        return;
    };

    println!("{}", result.summary());
    println!("{}", result.to_bencher_line());

    // `run_macro_bench` refuses to return at all unless the target holds every seeded row, so
    // reaching here is already the strongest assertion in the file. These restate it as a claim
    // rather than leaving it implicit in an error path nobody reads.
    assert_eq!(result.spec_rows, spec.rows);
    assert_eq!(result.rows_migrated, spec.rows);
    assert!(result.wall_clock > std::time::Duration::ZERO);
    assert!(
        result.rows_per_second > 0.0,
        "a completed migration cannot have zero throughput: {result:?}"
    );

    // A cold start is bounded by the run it is part of, and is not zero: something was measured.
    assert!(result.cold_start > std::time::Duration::ZERO);
    assert!(result.cold_start <= result.wall_clock);

    // Deliberately no upper bound on throughput or on cold start. `NFR-002`'s two seconds is
    // about a process cold start, which an in-process harness cannot observe (see
    // `MacroBenchResult::cold_start`), and `NFR-004`'s ratio is tier 3's subject. Asserting a
    // wall-clock threshold on a container under an unknown load is how a benchmark becomes a
    // flaky gate — the very thing `docs/BENCHMARKS.md` §3 argues against.

    let line = result.to_bencher_line();
    assert!(
        line.starts_with(&format!("test {BENCHER_NAME} ... bench:")),
        "{line}"
    );
    assert!(line.ends_with("ns/iter (+/- 0)"), "{line}");
}

/// `TST-102`: the benchmark is never the reason a run fails.
#[tokio::test]
async fn tst_102_the_macro_benchmark_skips_without_a_container_runtime() {
    // Runs unignored, on every machine, because "skips cleanly" is a property that only has
    // teeth if it is checked where there is no Docker — which is most laptops and every unit-test
    // CI job. Where there *is* a runtime the call would start containers, so the branch that
    // would do so is not taken.
    if ContainerRuntime::detect().is_ok() {
        return;
    }
    let outcome = run_macro_bench_or_skip(&spec()).await;
    assert!(
        matches!(outcome, Ok(None)),
        "a missing container runtime must skip, not fail: {outcome:?}"
    );
}
