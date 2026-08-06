//! Test fixtures: containers, schema/data generators, counter assertions, mock sessions.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! The shared harness every other crate's tests build on. It is a library, not a test suite: it
//! is consumed *by* tests, which means its own code is production code as far as the workspace
//! lints are concerned. Nothing here panics, unwraps or expects — a fixture returns a `Result`
//! and lets the calling test unwrap it, where a failure is the reporting mechanism.
//!
//! | Module | Provides | Requirement |
//! |---|---|---|
//! | [`containers`] | [`ClusterFixture`], [`OriginTarget`], [`Engine`], [`Capabilities`] | `TST-100`, `TST-002` |
//! | [`runtime`] | [`ContainerRuntime`] detection and [`skip_without_container_runtime!`] | `TST-102` |
//! | [`schema`] | [`TableSpec`], [`SchemaGen`], DDL rendering | `TST-100` |
//! | [`data`] | [`DataGen`], [`GeneratedRow`] | `TST-100`, `TST-101` |
//! | [`seed`] | [`Seed`], [`SeedGuard`] | `TST-101` |
//! | [`counters`] | [`CounterExpectation`], [`assert_counters!`], final-block parsing | `TST-100` |
//! | [`session`] | [`TestSession`], [`MockSession`], [`TestRow`] | `TST-100` |
//!
//! # The three things worth knowing before using it
//!
//! **Container tests skip, they do not fail.** `cargo test --workspace` must stay green on a
//! laptop with no Docker daemon (`TST-102`), so every containerised test opens with
//! [`skip_without_container_runtime!`] and returns early with an explanation when nothing
//! answers. `cargo xtask it` runs the same tests with the `--ignored` flag they carry.
//!
//! **Generated data is seeded, and a failure prints the seed.** Everything [`DataGen`] produces
//! is a pure function of a [`Seed`] (`TST-101`), and [`Seed::report_on_panic`] prints the seed
//! when a test unwinds, so a red CI log is reproducible with `CDM_TEST_SEED=…`.
//!
//! **Counter assertions state which accounting they mean.** cdm-rs keeps Java's interim/committed
//! two-level counters (`MET-004`), and reading the level you did not mean is the direct cause of
//! two Java defects cdm-rs deliberately does not reproduce (`MIG-004`, `ENG-008`).
//! [`CounterExpectation::check`] therefore takes the [`CounterView`](cdm_metrics::CounterView) as
//! an argument, and says so in the failure message when the other level would have passed. See
//! the [`counters`] module documentation.
//!
//! # What is a seam awaiting `cdm-cql`
//!
//! This crate may not depend on the driver — only `cdm-cql` may (`ARCHITECTURE.md` §3) — and
//! `cdm-cql`'s session type has not landed. So:
//!
//! * the container fixture starts a node, proves it is serving CQL with a native-protocol
//!   `OPTIONS` probe, and hands back a contact point. It does not create keyspaces or run DDL;
//! * the generators produce CQL *statements*, which anything holding a session can execute;
//! * [`TestSession`] is the one-method trait that runs them. [`MockSession`] implements it here;
//!   a real session implements it in `cdm-cql` in one small impl block, and nothing in this crate
//!   changes when it does.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `TST-100` — [`ClusterFixture`], [`OriginTarget`], [`SchemaGen`], [`DataGen`],
//!   [`CounterExpectation`], [`MockSession`]
//! - `TST-101` — [`Seed`], [`SeedGuard`], [`DataGen`]
//! - `TST-102` — [`ContainerRuntime`], [`skip_without_container_runtime!`], `cargo xtask it`

pub mod containers;
pub mod counters;
pub mod data;
pub mod runtime;
pub mod schema;
pub mod seed;
pub mod session;

pub use containers::{
    engines_under_test, Capabilities, ClusterFixture, Engine, FixtureOptions, Flavour,
    OriginTarget, CASSANDRA_VERSIONS, DEFAULT_NATIVE_PORT, ENGINES_ENV, SCYLLA_VERSIONS,
};
pub use counters::{counts, parse_final_block, parse_metrics_string, CounterExpectation};
pub use data::{quote, DataGen, DataGenOptions, GeneratedRow};
pub use runtime::{well_known_sockets, ContainerRuntime, NoContainerRuntime, RuntimeSource};
pub use schema::{
    create_keyspace_statement, type_slug, ColumnKind, ColumnSpec, SchemaGen, TableSpec,
    TableSpecBuilder, UdtSpec,
};
pub use seed::{Seed, SeedGuard, SEED_ENV};
pub use session::{apply_schema, seed_rows, MockSession, TestRow, TestSession};

/// Items the exported macros expand to.
///
/// Not a public API: a macro expands in the caller's crate, where `cdm_metrics` may not be in
/// scope and may not even be a dependency, so [`assert_counters!`] refers to everything it needs
/// through `$crate::reexport`.
#[doc(hidden)]
pub mod reexport {
    pub use cdm_metrics::CounterKind;
}

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
        // This crate is a library consumed *by* tests, so its non-test code is production code by
        // the lint's reckoning even though nothing here ever runs in a release build. Clippy
        // denies the panicking constructs where it lints; a source-level sweep also covers code
        // behind a `cfg` that a given clippy invocation does not compile.
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
                // `.expect("` rather than `.expect(`: the panicking `expect` always takes a
                // string literal, whereas `CounterExpectation::expect` — a builder method in this
                // very crate — does not, and a sweep that cannot tell them apart would have to be
                // suppressed, which defeats the point of having one.
                for construct in [
                    ".unwrap()",
                    ".expect(\"",
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
