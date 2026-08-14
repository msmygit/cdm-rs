//! The Java-parity differential harness (`TST-020`).
//!
//! `TST-020` asks for a harness that runs Java CDM and cdm-rs against the same seeded dataset and
//! asserts **byte-identical target state and identical counter blocks**. It has three parts, in
//! three places:
//!
//! | Part | Where | What it does |
//! |---|---|---|
//! | the corpus | `corpus.rs` | generates the seeded dataset: every CQL type, nesting depth 3, nulls, empty collections, edge-case values |
//! | the comparison | [`compare`] | decides whether the two runs agree, and says exactly how they do not |
//! | the wiring | `tests/differential/`, `.github/workflows/` | runs both implementations nightly and feeds one to the other |
//!
//! Only [`compare`] is here today. It deliberately knows nothing about how either target was
//! produced: it takes two snapshots and two counter blocks, whoever made them. That is what lets
//! it judge cdm-rs without using cdm-rs's own comparator to do it — see the module documentation
//! of [`compare`] for why that distinction is the whole point.

pub mod compare;
