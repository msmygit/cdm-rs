//! The Java-parity differential harness (`TST-020`).
//!
//! `TST-020` asks for a harness that runs cdm-rs and Java CDM against *the same seeded dataset*
//! and asserts byte-identical target state and identical counter blocks. It has two halves that
//! fail in completely different ways:
//!
//! * the **corpus** ([`corpus`]) — what the two implementations are pointed at. Its failure mode
//!   is silent: a type the corpus never generates is a type the harness never compares, and the
//!   run still reports "identical". Every gap is therefore named in
//!   [`Corpus::coverage`](corpus::Corpus::coverage) rather than left to be inferred from the
//!   schema;
//! * the **comparison engine** — how the two target states are reduced to a verdict. Its failure
//!   mode is loud: a wrong comparison reports a difference that is not there.
//!
//! This module owns the first. The comparison engine lands beside it as `compare.rs`, declared
//! here by whoever adds it.
//!
//! # Where the corpus lives
//!
//! The generator in [`corpus`] is the single source of truth. `tests/differential/` holds a
//! checked-in *rendering* of it — the DDL and the type-coverage manifest — so that the schema is
//! reviewable in a diff and loadable by a harness that has no Rust in it, and so that a change to
//! the corpus cannot slip through review unnoticed. [`corpus_root`] finds that directory, and
//! `tests/differential/README.md` says how to regenerate the files in it.

pub mod corpus;

pub use corpus::{
    Corpus, CorpusColumn, CorpusScale, CorpusTable, CoverageEntry, CoverageStatus, COVERAGE_FILE,
    KEYSPACE, SCHEMA_FILE,
};

use std::path::{Path, PathBuf};

/// The `tests/differential/` directory, which holds the checked-in rendering of the corpus.
///
/// Resolved from `CARGO_MANIFEST_DIR` rather than from the current directory, because `cargo test`
/// runs a test binary with the *package* root as its working directory and a test runner invoked
/// any other way does not.
#[must_use]
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("differential")
}
