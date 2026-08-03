//! The per-range tracing span and Java's thread label (`ENG-011`, `ENG-012`).
//!
//! # What replaces `ThreadContext`
//!
//! Java CDM correlates a log line with the range that produced it by writing a `ThreadLabel` into
//! log4j's `ThreadContext` at the top of `processPartitionRange` and removing it at the bottom.
//! That works only because a range owns a thread for its whole life. cdm-rs ranges are Tokio
//! tasks that move between threads, so `ENG-011` replaces the thread-local with a `tracing` span
//! that the range's future carries with it:
//!
//! | Field | Value |
//! |---|---|
//! | `run_id` | the run's [`RunId`], as it appears in `cdm_run_info` |
//! | `range_min` | the range's inclusive lower token bound |
//! | `range_max` | the range's inclusive upper token bound |
//! | `node_id` | this process's identity, so a fleet's logs demultiplex |
//! | `thread_label` | Java's label, present only when `logging.format = pretty` (`ENG-012`) |
//!
//! Everything a job logs, and every error the scheduler reports, is emitted inside this span.
//!
//! # The label is not `min:max`
//!
//! `SPEC.md` describes `ENG-012` as "the Java-compatible `min:max` label", which understates it.
//! `BaseJobSession.getThreadLabel(min, max)` right-*aligns* the upper bound in a field at least
//! twenty columns wide, so that labels line up in a terminal:
//!
//! ```text
//! -9223372036854775808:-4611686018427387904
//! 0:                 100
//! ```
//!
//! Twenty is the width of `Long.MIN_VALUE`, so for a Murmur3 full-ring run the padding is
//! invisible — which is exactly why it is easy to miss and worth reproducing deliberately. A
//! partial-ring run, or a `RandomPartitioner` run whose bounds reach 39 digits, produces labels
//! that differ from a naive `min:max` in every line. Java's lower bound goes through the same
//! formatter but is left-justified and then trimmed, which for a decimal integer is the identity;
//! [`java_thread_label`] reproduces the observable result rather than the redundant round trip.

use cdm_core::{RunId, TokenRange};
use tracing::Span;

/// The minimum width Java right-aligns the upper bound in — `Long.MIN_VALUE`'s digit count.
const JAVA_LABEL_MIN_WIDTH: usize = 20;

/// Java CDM's `ThreadLabel` for a range (`ENG-012`).
///
/// Reproduces `BaseJobSession.getThreadLabel(BigInteger, BigInteger)` exactly, padding included.
#[must_use]
pub fn java_thread_label(range: TokenRange) -> String {
    let min = range.min().to_string();
    let max = range.max().to_string();
    let width = min.len().max(max.len()).max(JAVA_LABEL_MIN_WIDTH);
    format!("{min}:{max:>width$}")
}

/// The span every range's processing runs inside (`ENG-011`, `ENG-012`).
///
/// `java_label` mirrors `logging.format = pretty`; when it is false the `thread_label` field is
/// declared but never recorded, so structured formats carry the four machine-readable fields and
/// nothing redundant.
#[must_use]
pub fn range_span(run_id: RunId, range: TokenRange, node_id: &str, java_label: bool) -> Span {
    // The token bounds are `i128`, which `tracing` has no primitive value type for; recording
    // them with `Display` keeps them exact, which `as i64` would not.
    let span = tracing::info_span!(
        "cdm.range",
        run_id = run_id.as_i64(),
        range_min = %range.min(),
        range_max = %range.max(),
        node_id = %node_id,
        thread_label = tracing::field::Empty,
    );
    if java_label {
        span.record("thread_label", java_thread_label(range));
    }
    span
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::*;

    /// A subscriber that records span fields, so `ENG-011` can be asserted on the real span
    /// rather than on a re-implementation of it.
    ///
    /// A span built with no subscriber installed is disabled and carries no metadata at all, so
    /// there is no way to check its fields without one. `tracing-subscriber` has no capture
    /// layer suitable for assertions, and this is thirty lines.
    #[derive(Debug, Clone, Default)]
    pub(crate) struct CapturingSubscriber {
        fields: Arc<Mutex<BTreeMap<String, String>>>,
        name: Arc<Mutex<Option<&'static str>>>,
        depth: Arc<AtomicIsize>,
    }

    impl CapturingSubscriber {
        pub(crate) fn field(&self, name: &str) -> Option<String> {
            self.fields.lock().ok()?.get(name).cloned()
        }

        pub(crate) fn span_name(&self) -> Option<&'static str> {
            *self.name.lock().ok()?
        }

        /// How many spans are entered on this thread right now.
        ///
        /// `tracing::Span::current()` would be the obvious way to ask "am I inside the range
        /// span?", but it goes through `Subscriber::current_span`, whose type is not re-exported
        /// by the `tracing` facade. Counting `enter`/`exit` answers the same question with the
        /// API that is available.
        pub(crate) fn entered_depth(&self) -> isize {
            self.depth.load(Ordering::SeqCst)
        }
    }

    struct Collector<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for Collector<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attrs: &Attributes<'_>) -> Id {
            if let Ok(mut name) = self.name.lock() {
                *name = Some(attrs.metadata().name());
            }
            if let Ok(mut fields) = self.fields.lock() {
                attrs.record(&mut Collector(&mut fields));
            }
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, values: &Record<'_>) {
            if let Ok(mut fields) = self.fields.lock() {
                values.record(&mut Collector(&mut fields));
            }
        }

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
        fn event(&self, _event: &Event<'_>) {}

        fn enter(&self, _span: &Id) {
            self.depth.fetch_add(1, Ordering::SeqCst);
        }

        fn exit(&self, _span: &Id) {
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn eng_011_the_range_span_carries_the_run_range_and_node_identity() {
        let captured = CapturingSubscriber::default();
        tracing::subscriber::with_default(captured.clone(), || {
            let _span = range_span(RunId::from_raw(42), range(-100, 900), "node-7", false);
        });

        assert_eq!(captured.span_name(), Some("cdm.range"));
        assert_eq!(captured.field("run_id").as_deref(), Some("42"));
        assert_eq!(captured.field("range_min").as_deref(), Some("-100"));
        assert_eq!(captured.field("range_max").as_deref(), Some("900"));
        assert_eq!(captured.field("node_id").as_deref(), Some("node-7"));
    }

    #[test]
    fn eng_011_the_token_bounds_are_exact_beyond_the_range_of_an_i64() {
        let captured = CapturingSubscriber::default();
        let wide = range(0, i128::from(u64::MAX) * 4);
        tracing::subscriber::with_default(captured.clone(), || {
            let _span = range_span(RunId::from_raw(1), wide, "n", false);
        });
        assert_eq!(
            captured.field("range_max").as_deref(),
            Some("73786976294838206460")
        );
    }

    #[test]
    fn eng_012_the_java_label_is_recorded_only_for_the_pretty_format() {
        let pretty = CapturingSubscriber::default();
        tracing::subscriber::with_default(pretty.clone(), || {
            let _span = range_span(RunId::from_raw(1), range(0, 100), "n", true);
        });
        assert_eq!(
            pretty.field("thread_label").as_deref(),
            Some("0:                 100")
        );

        let structured = CapturingSubscriber::default();
        tracing::subscriber::with_default(structured.clone(), || {
            let _span = range_span(RunId::from_raw(1), range(0, 100), "n", false);
        });
        assert_eq!(structured.field("thread_label"), None);
    }

    #[test]
    fn eng_012_the_upper_bound_is_right_aligned_in_at_least_twenty_columns() {
        // Java: String.format("%20s", "100") — the padding is why the labels line up.
        let label = java_thread_label(range(0, 100));
        assert_eq!(label, "0:                 100");
        assert_eq!(label.len(), "0:".len() + JAVA_LABEL_MIN_WIDTH);
    }

    #[test]
    fn eng_012_a_full_murmur3_ring_label_pads_the_positive_bound_by_one_column() {
        // The trap `SPEC.md`'s "`min:max` label" hides, in the single most common case there is:
        // `Long.MIN_VALUE` is twenty characters and `Long.MAX_VALUE` is nineteen, so a full-ring
        // label carries exactly one space that a naive `format!("{min}:{max}")` would not.
        let label = java_thread_label(TokenRange::MURMUR3_FULL);
        assert_eq!(label, "-9223372036854775808: 9223372036854775807");
        assert_ne!(label, "-9223372036854775808:9223372036854775807");
    }

    #[test]
    fn eng_012_a_bound_wider_than_twenty_columns_widens_the_field() {
        // RandomPartitioner tokens reach 39 digits, past Java's twenty-column minimum.
        let label = java_thread_label(TokenRange::RANDOM_FULL);
        let (min, max) = label.split_once(':').unwrap();
        assert_eq!(min, "0");
        assert_eq!(max.len(), "170141183460469231731687303715884105727".len());
        assert_eq!(max.trim_start(), "170141183460469231731687303715884105727");
    }

    #[test]
    fn eng_012_a_negative_lower_bound_is_never_trimmed_away() {
        // Java left-justifies the lower bound and trims it, which for a decimal integer is the
        // identity — including the sign.
        assert!(java_thread_label(range(-5, 5)).starts_with("-5:"));
    }
}
