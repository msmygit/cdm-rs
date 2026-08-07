//! The migrate job: read a token range from the origin and write it to the target.
//!
//! This is what cdm-rs is for. Everything else — the planner, the scheduler, the codecs, the
//! features, the tracking store — exists to put rows through the loop in [`job`].
//!
//! # The pieces
//!
//! | Module | Responsibility | Requirements |
//! |---|---|---|
//! | [`settings`] | the coerced batch size and the flush threshold | `MIG-004`, `MIG-021`, `MIG-022`, `MIG-041` |
//! | [`plan`] | everything resolved once, before the first row | `ARCHITECTURE.md` §5.5 |
//! | [`job`] | the row loop | `MIG-001`..`MIG-003`, `MIG-041`, `SCH-009` |
//! | [`buffer`] | batching, the flush threshold, the `WRITE` credit | `MIG-004`, `MIG-005`, `MIG-020`, `MIG-022` |
//! | [`counter`] | the counter delta and why nothing about it is retried | `MIG-030`..`MIG-032`, `CON-011`, `CON-012` |
//! | [`sink`] | where a bound write goes, including nowhere | `MIG-041` |
//! | [`counters`] | the five counter tokens, resolved at startup | `MET-002`, `MET-003` |
//!
//! # Three things worth knowing before reading the code
//!
//! **The flush threshold fires.** `MIG-004` gives a formula that Java computes correctly and then
//! compares against a counter that is structurally always zero, so Java flushes once per token
//! range and buffers the whole range in memory. cdm-rs compares the level that is actually
//! incremented. `--compat-java` does not restore the bug. See [`settings`].
//!
//! **Counters are at-most-once, at four independent levels**, none of which is a runtime `if` that
//! could be deleted by accident. See [`counter`].
//!
//! **Zero-copy survives the whole path.** A value read off the response frame is bound, batched and
//! written without ever being decoded, and the test that proves it compares pointers rather than
//! bytes. See [`job`].

pub mod buffer;
pub mod counter;
pub mod counters;
pub mod job;
pub mod plan;
pub mod settings;
pub mod sink;

#[cfg(test)]
pub(crate) mod testfixtures;

pub use buffer::WriteBuffer;
pub use counter::{CounterColumn, CounterDeltas, CounterPlan};
pub use counters::MigrateCounters;
pub use job::MigrateJob;
pub use plan::{MigrateFeatures, MigratePlan};
pub use settings::{BatchCoercion, MigrateSettings};
pub use sink::{CqlSink, DryRunSink, WriteSink};
