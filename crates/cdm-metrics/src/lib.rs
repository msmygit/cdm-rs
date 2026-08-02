//! Counter registry and the Java-format reporter (`MET-001`..`MET-006`).
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! The thirteen counters every cdm-rs job keeps, and the two strings other people's tooling reads
//! them out of:
//!
//! * [`CounterKind`] — the counter vocabulary of `MET-001`, in Java's declaration order, which is
//!   also the rendering order;
//! * [`JobCounters`] — a lock-free registry of `AtomicU64` pairs, registered per job (`MET-002`),
//!   with Java's interim/committed two-level accounting (`MET-004`);
//! * [`Counter`] — proof that a counter is registered, resolved once at startup so that the hot
//!   path cannot fail (`MET-003`);
//! * [`JobCounters::metrics`] and [`JobCounters::final_block`] — the metrics string (`MET-005`)
//!   and the final block (`MET-006`), byte for byte as Java emits them.
//!
//! # These strings are a public contract
//!
//! `COMPAT-004` requires the metrics string and the final block to stay character-identical to
//! Java's. They are grepped by `SIT/cdm-assert.sh`, parsed by users' scripts, and stored in
//! `cdm_run_info.run_info` and `cdm_run_details.run_info`, where Java CDM reads them back when it
//! decides whether a run may be resumed (`TRK-030`). A changed separator or capital letter is a
//! silent breakage, so [`CounterKind::title_case`] is a written-down table rather than a call to
//! a case-conversion helper, and every rule has a test that a refactor would have to delete
//! rather than merely disturb.
//!
//! # Usage
//!
//! ```
//! use cdm_core::{JobKind, RunId};
//! use cdm_metrics::{CounterKind, JobCounters};
//!
//! // Once per run, and once per range.
//! let run = JobCounters::new(JobKind::Migrate);
//! let range = JobCounters::new(JobKind::Migrate);
//!
//! // Once, at startup: an unregistered counter fails here, never on a row (MET-003).
//! let read = range.counter(CounterKind::Read)?;
//! let write = range.counter(CounterKind::Write)?;
//! let passed = range.counter(CounterKind::PartitionsPassed)?;
//!
//! // The hot path: one relaxed atomic add, no error branch.
//! range.increment(read);
//! range.increment(write);
//!
//! // The range completed: fold interim into committed, record `run_info`, merge into the run.
//! range.increment(passed);
//! range.flush();
//! assert_eq!(
//!     range.run_info(),
//!     "Read: 1; Write: 1; Skipped: 0; Error: 0; Partitions Passed: 1; Partitions Failed: 0",
//! );
//! run.add(&range)?;
//!
//! // The run completed.
//! run.log_final_block(Some(RunId::from_raw(1_712_345_678_901_234)));
//! # Ok::<(), cdm_core::CdmError>(())
//! ```
//!
//! # Not here yet
//!
//! The new observability of `MET-010`..`MET-033` — throughput and latency instruments, the
//! Prometheus and OTLP exporters, the event bus, the terminal UI — arrives in PRs #36..#39. This
//! crate is deliberately the parity core and nothing else for now.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `MET-001` — [`CounterKind`]
//! - `MET-002` — [`registered_counters`], [`CounterKind::is_registered_for`]
//! - `MET-003` — [`Counter`], [`JobCounters::counter`]
//! - `MET-004` — [`CounterView`], [`JobCounters::flush`], [`JobCounters::add`]
//! - `MET-005` — [`JobCounters::metrics`], [`JobCounters::run_info`]
//! - `MET-006` — [`JobCounters::final_block`], [`JobCounters::log_final_block`]

mod counter;
mod registry;
mod report;

pub use counter::{registered_counters, CounterKind};
pub use registry::{Counter, CounterView, JobCounters};
pub use report::{FINAL_BLOCK_RULE, METRIC_SEPARATOR};

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
            // `src/snapshots/*.snap` lives here too, and is data, not code.
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
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
