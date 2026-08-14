//! The Java-parity differential harness (`TST-020`).
//!
//! `TST-020` asks for a harness that runs cdm-rs and Java CDM against *the same seeded dataset*
//! and asserts byte-identical target state and identical counter blocks. It has three parts, and
//! they fail in completely different ways — which is why they are separate modules:
//!
//! | Part | Where | Failure mode |
//! |---|---|---|
//! | the corpus | [`corpus`] | **silent**: a type the corpus never generates is a type the harness never compares, and the run still reports "identical" |
//! | the comparison | [`compare`] | **loud**: a wrong comparison reports a difference that is not there |
//! | the wiring | `xtask differential`, `.github/workflows/differential.yml` | runs both implementations nightly and feeds one to the other |
//!
//! Because the corpus fails silently, every gap in it is named in
//! [`Corpus::coverage`](corpus::Corpus::coverage) rather than left to be inferred from the schema.
//!
//! [`compare`] deliberately knows nothing about how either target was produced: it takes two
//! snapshots and two counter blocks, whoever made them. That is what lets it judge cdm-rs without
//! using cdm-rs's own comparator to do it — see its module documentation for why that distinction
//! is the whole point.
//!
//! # Where the corpus lives
//!
//! The generator in [`corpus`] is the single source of truth. `tests/differential/` holds a
//! checked-in *rendering* of it — the DDL and the type-coverage manifest — so that the schema is
//! reviewable in a diff and loadable by a harness that has no Rust in it, and so that a change to
//! the corpus cannot slip through review unnoticed. [`corpus_root`] finds that directory, and
//! `tests/differential/README.md` says how to regenerate the files in it.

pub mod compare;
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
