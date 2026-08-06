//! Scheduler, rate limiting, backpressure, failure isolation and the built-in jobs.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `TOK-001`..`TOK-010` — [`planner`], the token-range planner
//! - `ENG-001`..`ENG-014` — [`scheduler`], the work-stealing execution engine
//! - `MIG-001`..`MIG-005`, `MIG-020`..`MIG-022`, `MIG-030`..`MIG-032`, `MIG-041` — [`migrate`],
//!   the migrate job
//! - `VAL-001`..`VAL-012`, `VAL-016`, `VAL-017` — [`jobs::validate`], the diff job
//! - `GRD-001`..`GRD-004` — [`jobs::guardrail`], the guardrail job
//!
//! # How the two halves fit together
//!
//! [`planner`] decides *what* to process: it splits the ring into token ranges and shuffles them
//! into a [`TokenPlan`], deterministically and with no I/O. [`scheduler`] decides *how*: it runs
//! that plan across `perfops.workers` Tokio tasks, paces them, bounds their memory, isolates
//! their failures and accounts for them.
//!
//! Neither half knows what a row is. The whole of the job surface is
//! [`scheduler::RangeProcessor`] — "given a range, process it and report
//! counters" — which migrate (`MIG`), validate (`VAL`) and guardrail (`GRD`) implement in
//! PRs #21–#24.
//!
//! # Status
//!
//! The planner, the scheduler, the validate job ([`jobs::validate`]) and the guardrail job
//! ([`jobs::guardrail`]) are implemented. Migrate lands in the pull request listed in
//! [`docs/ROADMAP.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/ROADMAP.md).

pub mod jobs;
pub mod migrate;
pub mod planner;
pub mod scheduler;

pub use migrate::{MigrateJob, MigratePlan, MigrateSettings};
pub use planner::{
    subdivide_for_rerun, ClusterTopology, InMemoryTopology, MemoryEnvelope, Partitioner,
    PlanReport, PlanStrategy, PlannedRange, Planner, PlannerSettings, RingSegment, SizeEstimate,
    SpanBucket, TokenPlan,
};
pub use scheduler::{
    NoopObserver, RangeContext, RangeObserver, RangeOutcome, RangeProcessor, RangeVerdict,
    RateLimiter, RunControl, RunReport, RuntimeLimits, Scheduler, SchedulerSettings, StopReason,
};

/// The version of this crate, as reported by `cdm version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn err_004_no_production_path_in_this_crate_can_panic() {
        // Clippy denies `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!` workspace-wide, but
        // only where it lints; a source-level sweep also covers code behind a `cfg` that a given
        // clippy invocation does not compile.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    stack.push(entry.unwrap().path());
                }
                continue;
            }
            // A `tests.rs` is a `#[cfg(test)] mod tests;` in its own file: it never carries the
            // in-file marker this sweep splits on, and every line of it is a test body.
            if path.file_stem().is_some_and(|stem| stem == "tests") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Everything from the test module onwards is exempt.
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (offset, line) in production.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for construct in [
                    ".unwrap()",
                    ".expect(",
                    "panic!(",
                    "todo!(",
                    "unimplemented!(",
                ] {
                    if code.contains(construct) {
                        offenders.push(format!("{}:{}", path.display(), offset + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "panicking constructs found: {offenders:?}"
        );
    }
}
