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

use crate::differential::compare::SnapshotSpec;

/// What to read back from one corpus table, taken from the corpus's own column metadata.
///
/// The join between the two halves of this module, and the reason it is here rather than in the
/// runner: the corpus states what it built and the comparator selects exactly that, so the
/// translation between them belongs to neither and must exist only once. `xtask differential` and
/// `tests/differential_corpus_it.rs` both go through this function, which is what makes the
/// container test a test of what the nightly actually does.
///
/// Writetime and TTL are selected where — and only where —
/// [`CorpusColumn::timestamp_eligible`](corpus::CorpusColumn::timestamp_eligible) says the server
/// will answer with a `bigint`. That rule was *measured* against `cassandra:5.0.9`, and a second
/// rule inferred here would be a second source of truth, and the one nobody measured. Note in
/// particular that a column which is not timestamp-eligible still has its value compared byte for
/// byte; only per-cell metadata the server will not report goes unasked.
#[must_use]
pub fn snapshot_spec(table: &CorpusTable) -> SnapshotSpec {
    let mut spec = SnapshotSpec::new(table.spec().keyspace(), table.spec().table());
    for column in table.key_columns() {
        spec = spec.key_column(column.name());
    }
    for column in table.value_columns() {
        spec = if column.timestamp_eligible() {
            spec.value_column(column.name())
        } else {
            spec.value_column_without_timestamps(column.name())
        };
    }
    spec
}

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

#[cfg(test)]
mod tests {
    // A failed assertion *is* the reporting mechanism in a test; the no-panic rule (`ERR-004`)
    // protects production paths, not test bodies.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The smoke corpus: the same schema and the same edge rows as the full one, fewer filler rows.
    fn corpus() -> Corpus {
        Corpus::smoke(crate::Seed::new(7)).unwrap()
    }

    /// The projection asks for `WRITETIME`/`TTL` exactly where the corpus says the server answers
    /// with a `bigint`, and nowhere else.
    ///
    /// The rule itself is deliberately not restated here — restating it is the bug this guards
    /// against. The corpus measured which columns are eligible against `cassandra:5.0.9`; this
    /// asserts only that the runner asked the corpus rather than deciding for itself.
    #[test]
    fn tst_020_the_snapshot_spec_takes_its_timestamps_from_the_corpus() {
        let corpus = corpus();
        for table in corpus.tables() {
            let spec = snapshot_spec(table);
            let statement = spec.select_statement().unwrap();

            for column in table.key_columns() {
                assert!(
                    !statement.contains(&format!("WRITETIME(\"{}\")", column.name())),
                    "a primary-key column is not selectable with WRITETIME and the server rejects \
                     the whole SELECT for asking: {statement}"
                );
            }
            for column in table.value_columns() {
                let asked = statement.contains(&format!("WRITETIME(\"{}\")", column.name()));
                assert_eq!(
                    asked,
                    column.timestamp_eligible(),
                    "`{}` is timestamp_eligible = {} but the projection {} its WRITETIME",
                    column.name(),
                    column.timestamp_eligible(),
                    if asked { "asks for" } else { "omits" }
                );
            }
        }
    }

    /// Every column of every corpus table is read, whether or not its writetime can be.
    ///
    /// An ineligible column is not an unread column: its *value* is still compared byte for byte,
    /// and only the per-cell metadata the server will not report goes unasked.
    #[test]
    fn tst_020_every_corpus_column_is_projected() {
        let corpus = corpus();
        for table in corpus.tables() {
            let spec = snapshot_spec(table);
            assert_eq!(spec.key_columns().len(), table.key_columns().len());
            assert_eq!(spec.value_columns().len(), table.value_columns().len());

            let statement = spec.select_statement().unwrap();
            for column in table.columns() {
                assert!(
                    statement.contains(&format!("\"{}\"", column.name())),
                    "`{}` is a column of {} and is not in the snapshot projection: {statement}",
                    column.name(),
                    table.qualified_name()
                );
            }
        }
    }
}
