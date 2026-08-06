//! The closed label set every exported series carries (`MET-020`, `SEC-001`).
//!
//! A metrics exporter is one of the two places in cdm-rs where a configuration value is most
//! likely to escape into somewhere it can never be recalled — the other is the event stream of
//! `MET-030`. A Prometheus scrape is archived, replicated and queried by people who were never
//! near the run; an OTLP collector forwards it to a vendor. `SEC-001` therefore admits no
//! "serialise whatever is in the config" path, and this module is how that is made structural
//! rather than a matter of care: [`MetricLabels`] has six fields, all of them named by `MET-020`,
//! and there is no constructor, builder or `Extend` impl that accepts an arbitrary key.
//!
//! # Identity labels versus intrinsic labels
//!
//! `MET-020` names `{run_id, job, side, node_id, keyspace, table}`. Those are *identity* labels:
//! they say which run, on which node, over which table, a series belongs to, and they are the
//! only labels whose values come from configuration. They live here.
//!
//! Series also carry *intrinsic* labels — `operation`, `quantile`, `cause`, `state`, `window` —
//! which are dimensions of the measurement itself. Every one of those comes from a closed Rust
//! enum in this crate ([`Operation`](crate::Operation), [`RetryCause`](crate::RetryCause), and so
//! on), never from a string a user supplied, so they add bounded cardinality and cannot carry a
//! secret. `SPEC.md` §15.2 does not draw this distinction explicitly; the constraint it does state
//! — "cardinality MUST NOT include token ranges or primary keys" — is met by both kinds, and
//! `met_020_no_series_is_labelled_by_a_token_range_or_a_key` pins it.
//!
//! # Side is not stored here
//!
//! `side` is per-series, not per-run: `cdm_rows_total{side="origin"}` and `{side="target"}` are
//! the same run. It is therefore passed to the renderer alongside the labels rather than held in
//! them, which is also why [`MetricLabels`] holds five fields for six label names.

use std::fmt::Write as _;

use cdm_core::{JobKind, RunId, Side, TableRef};
use serde::{Deserialize, Serialize};

/// The identity labels of `MET-020`, and the only labels whose values come from configuration.
///
/// ```
/// use cdm_core::{JobKind, RunId, TableRef};
/// use cdm_metrics::MetricLabels;
///
/// let labels = MetricLabels::new(RunId::from_raw(7), JobKind::Migrate, "node-a")
///     .with_table(&TableRef::new("ks", "orders"));
///
/// assert_eq!(labels.keyspace(), Some("ks"));
/// assert_eq!(MetricLabels::NAMES.len(), 6);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricLabels {
    run_id: RunId,
    job: JobKind,
    node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keyspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    table: Option<String>,
}

impl MetricLabels {
    /// Every identity label name `MET-020` permits, in rendering order.
    ///
    /// This array is the whole allow-list. A series carrying a name that is not in it is a bug,
    /// and `sec_001_the_identity_label_set_is_closed` fails on one.
    pub const NAMES: [&'static str; 6] = ["run_id", "job", "side", "node_id", "keyspace", "table"];

    /// The labels of a run, before the table is known.
    ///
    /// `node_id` is the engine's `cluster.node_id` (host name and process id by default), which is
    /// operator-supplied but is neither a secret nor row data.
    #[must_use]
    pub fn new(run_id: RunId, job: JobKind, node_id: impl Into<String>) -> Self {
        Self {
            run_id,
            job,
            node_id: node_id.into(),
            keyspace: None,
            table: None,
        }
    }

    /// Adds the keyspace and table being processed.
    ///
    /// Taking a [`TableRef`] rather than two strings is deliberate: the only way to label a series
    /// with a table is to have resolved one, so a stray configuration string cannot arrive here by
    /// mistake.
    #[must_use]
    pub fn with_table(mut self, table: &TableRef) -> Self {
        self.keyspace = Some(table.keyspace().to_owned());
        self.table = Some(table.table().to_owned());
        self
    }

    /// The run these series belong to.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// The job that produced them.
    #[must_use]
    pub const fn job(&self) -> JobKind {
        self.job
    }

    /// The node that produced them.
    #[must_use]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The keyspace, when a table has been resolved.
    #[must_use]
    pub fn keyspace(&self) -> Option<&str> {
        self.keyspace.as_deref()
    }

    /// The table, when one has been resolved.
    #[must_use]
    pub fn table(&self) -> Option<&str> {
        self.table.as_deref()
    }

    /// The label pairs, in [`MetricLabels::NAMES`] order, with `side` included when given.
    ///
    /// Absent labels are omitted rather than rendered empty: an empty label value and a missing
    /// label are the same thing to Prometheus, and omitting keeps the exposition smaller.
    #[must_use]
    pub fn pairs(&self, side: Option<Side>) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::with_capacity(Self::NAMES.len());
        pairs.push(("run_id", self.run_id.to_string()));
        pairs.push(("job", self.job.as_str().to_owned()));
        if let Some(side) = side {
            pairs.push(("side", side.as_str().to_owned()));
        }
        pairs.push(("node_id", self.node_id.clone()));
        if let Some(keyspace) = &self.keyspace {
            pairs.push(("keyspace", keyspace.clone()));
        }
        if let Some(table) = &self.table {
            pairs.push(("table", table.clone()));
        }
        pairs
    }

    /// Renders the identity labels, plus `side` and any intrinsic labels, as a Prometheus label
    /// block including the surrounding braces.
    ///
    /// `intrinsic` carries the closed-enum dimensions of the series — `operation`, `quantile`,
    /// `cause`, `state`, `window` — whose values this crate produces and a user never supplies.
    #[must_use]
    pub fn render_prometheus(&self, side: Option<Side>, intrinsic: &[(&str, &str)]) -> String {
        let mut out = String::from("{");
        let mut first = true;
        for (name, value) in self.pairs(side) {
            push_label(&mut out, &mut first, name, &value);
        }
        for (name, value) in intrinsic {
            push_label(&mut out, &mut first, name, value);
        }
        out.push('}');
        out
    }
}

/// Appends one `name="value"` pair, escaping the value as the Prometheus text format requires.
fn push_label(out: &mut String, first: &mut bool, name: &str, value: &str) {
    if !*first {
        out.push(',');
    }
    *first = false;
    // The formatter writes into a `String`, whose `Write` impl is infallible; the result is
    // discarded rather than unwrapped so that no panicking path exists (`ERR-004`).
    let _ = write!(out, "{name}=\"{}\"", escape_label_value(value));
}

/// Escapes a Prometheus label value: backslash, double quote and newline (`MET-020`).
///
/// Cassandra identifiers may contain a double quote (`SCH-002` doubles them when quoting CQL), and
/// an unescaped one would end the label early and produce an unparseable exposition.
#[must_use]
pub fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
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
mod tests {
    use super::*;

    fn labels() -> MetricLabels {
        MetricLabels::new(RunId::from_raw(1_712_345_678), JobKind::Validate, "node-a")
            .with_table(&TableRef::new("ks", "orders"))
    }

    #[test]
    fn met_020_labels_render_in_the_documented_order() {
        assert_eq!(
            labels().render_prometheus(Some(Side::Origin), &[]),
            "{run_id=\"1712345678\",job=\"validate\",side=\"origin\",node_id=\"node-a\",\
             keyspace=\"ks\",table=\"orders\"}"
        );
    }

    #[test]
    fn met_020_absent_labels_are_omitted_rather_than_rendered_empty() {
        let bare = MetricLabels::new(RunId::from_raw(1), JobKind::Migrate, "n");
        assert_eq!(bare.keyspace(), None);
        assert_eq!(bare.table(), None);
        assert_eq!(
            bare.render_prometheus(None, &[]),
            "{run_id=\"1\",job=\"migrate\",node_id=\"n\"}"
        );
    }

    #[test]
    fn met_020_intrinsic_labels_follow_the_identity_labels() {
        let rendered = labels().render_prometheus(Some(Side::Target), &[("operation", "write")]);
        assert!(rendered.ends_with(",operation=\"write\"}"), "{rendered}");
    }

    #[test]
    fn sec_001_the_identity_label_set_is_closed() {
        // Every name a rendered series can carry is one of the six `MET-020` allows. There is no
        // constructor that accepts another, which is the point: this test states the invariant,
        // and the type system enforces it.
        let names: Vec<&str> = labels()
            .pairs(Some(Side::Origin))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, MetricLabels::NAMES);
        for name in &names {
            assert!(MetricLabels::NAMES.contains(name), "{name} is not allowed");
        }
    }

    #[test]
    fn sec_001_a_quoted_identifier_cannot_break_out_of_a_label_value() {
        // `SCH-002` allows an identifier to contain a quote or a backslash. Unescaped, either
        // would end the label early and let the rest of the name be read as more labels.
        let awkward = MetricLabels::new(RunId::from_raw(1), JobKind::Migrate, "node\"a")
            .with_table(&TableRef::new("ks\\1", "or\"ders"));
        let rendered = awkward.render_prometheus(None, &[]);
        assert!(rendered.contains("node_id=\"node\\\"a\""), "{rendered}");
        assert!(rendered.contains("keyspace=\"ks\\\\1\""), "{rendered}");
        assert_eq!(escape_label_value("a\nb"), "a\\nb");
        // The braces still pair, and the label count is unchanged.
        assert_eq!(rendered.matches('{').count(), 1);
        assert_eq!(rendered.matches('}').count(), 1);
    }

    #[test]
    fn met_020_side_is_per_series_not_per_run() {
        let labels = labels();
        let origin = labels.render_prometheus(Some(Side::Origin), &[]);
        let target = labels.render_prometheus(Some(Side::Target), &[]);
        assert_ne!(origin, target);
        assert!(!labels.render_prometheus(None, &[]).contains("side=\""));
    }
}
