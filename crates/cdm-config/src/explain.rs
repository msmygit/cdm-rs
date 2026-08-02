//! Explaining and diffing configurations (`CFG-028`, `CFG-029`).
//!
//! The `cdm config explain` and `cdm config diff` **commands** land with the CLI in PR #10; what
//! is here is everything those commands need, so that the CLI is argument parsing and rendering
//! only, and so that the HTTP API and the config-builder UI can offer the same two answers
//! without going through a subprocess.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

use crate::loader::{LoadOutcome, Source};
use crate::meta::PropertyMeta;
use crate::model::CdmConfig;
use crate::registry::PropertyRegistry;

/// Everything `cdm config explain <key>` prints (`CFG-028`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    /// The canonical property name.
    pub canonical: String,
    /// The legacy `spark.cdm.*` names it also answers to.
    pub legacy: Vec<String>,
    /// The property's type.
    pub kind: String,
    /// One line describing what the property does.
    pub summary: String,
    /// The whole documentation of the property.
    pub description: String,
    /// The built-in default, redacted if the property is a secret.
    pub default_value: Option<String>,
    /// The unit the value is expressed in.
    pub unit: Option<String>,
    /// The value in force, redacted if the property is a secret.
    pub effective_value: Option<String>,
    /// Which layer supplied the effective value.
    pub source: Source,
    /// Where within that layer it was written.
    pub location: Option<String>,
}

impl fmt::Display for Explanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.canonical)?;
        writeln!(f, "  {}", self.summary)?;
        writeln!(f, "  type:      {}", self.kind)?;
        if let Some(unit) = &self.unit {
            writeln!(f, "  unit:      {unit}")?;
        }
        writeln!(
            f,
            "  default:   {}",
            self.default_value.as_deref().unwrap_or("—")
        )?;
        writeln!(
            f,
            "  effective: {}",
            self.effective_value.as_deref().unwrap_or("—")
        )?;
        write!(f, "  source:    {}", self.source)?;
        if let Some(location) = &self.location {
            write!(f, " ({location})")?;
        }
        if !self.legacy.is_empty() {
            write!(f, "\n  also:      {}", self.legacy.join(", "))?;
        }
        Ok(())
    }
}

/// Explains one property against the outcome of a load (`CFG-028`).
///
/// `key` may be the canonical name or any legacy alias. Returns `None` when nothing by that name
/// exists — the caller pairs that with
/// [`PropertyRegistry::closest_match`](crate::PropertyRegistry::closest_match).
pub fn explain(key: &str, outcome: &LoadOutcome) -> Option<Explanation> {
    let registry = PropertyRegistry::global();
    let meta = registry.resolve(key)?;
    let effective = outcome
        .config
        .as_ref()
        .and_then(|config| render(config, meta));

    Some(Explanation {
        canonical: meta.canonical.clone(),
        legacy: meta.legacy.clone(),
        kind: meta.kind.to_string(),
        summary: meta.summary.clone(),
        description: meta.description.clone(),
        default_value: meta.displayed_default(),
        unit: meta.unit.map(str::to_owned),
        effective_value: effective,
        source: outcome
            .sources
            .get(&meta.canonical)
            .cloned()
            .unwrap_or(Source::Defaults),
        location: outcome.locations.get(&meta.canonical).cloned(),
    })
}

/// One property that differs between two configurations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyChange {
    /// The canonical property name.
    pub canonical: String,
    /// Its value on the left, redacted if the property is a secret.
    pub left: Option<String>,
    /// Its value on the right, redacted if the property is a secret.
    pub right: Option<String>,
}

/// The normalised difference between two configurations (`CFG-029`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    /// The properties that differ, in canonical order.
    pub changes: Vec<PropertyChange>,
}

impl ConfigDiff {
    /// Whether the two configurations mean the same thing.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

impl fmt::Display for ConfigDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.changes.is_empty() {
            return f.write_str("the two configurations are equivalent");
        }
        for (index, change) in self.changes.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(
                f,
                "{}\n  - {}\n  + {}",
                change.canonical,
                change.left.as_deref().unwrap_or("—"),
                change.right.as_deref().unwrap_or("—")
            )?;
        }
        Ok(())
    }
}

/// Compares two configurations property by property (`CFG-029`).
///
/// The comparison is *semantic*: it walks the registry rather than the documents, so key
/// ordering, section nesting, whether a value was written in TOML or in a `.properties` file, and
/// which spelling of a property name was used are all invisible. Properties that are at their
/// default on both sides do not appear, because a difference in how a default was arrived at is
/// not a difference in what will happen.
///
/// Secrets are compared but never printed: the change shows `***` on both sides, which still
/// tells the operator *that* the credential differs.
pub fn diff(left: &CdmConfig, right: &CdmConfig) -> ConfigDiff {
    let registry = PropertyRegistry::global();
    let mut changes = Vec::new();
    let mut seen = BTreeSet::new();

    for meta in registry.all() {
        if !seen.insert(meta.canonical.clone()) {
            continue;
        }
        let (a, b) = (raw(left, meta), raw(right, meta));
        if a == b {
            continue;
        }
        changes.push(PropertyChange {
            canonical: meta.canonical.clone(),
            left: render(left, meta),
            right: render(right, meta),
        });
    }

    ConfigDiff { changes }
}

/// The raw JSON value of a property, used for comparison.
fn raw(config: &CdmConfig, meta: &PropertyMeta) -> Option<Value> {
    let json = serde_json::to_value(config).ok()?;
    let mut node = &json;
    for segment in meta.canonical.split('.') {
        node = node.get(segment)?;
    }
    match node {
        Value::Null => None,
        other => Some(other.clone()),
    }
}

/// The displayable value of a property, redacted if it is a secret.
fn render(config: &CdmConfig, meta: &PropertyMeta) -> Option<String> {
    let value = raw(config, meta)?;
    if meta.secret {
        return Some(crate::secret::REDACTED.to_owned());
    }
    Some(match value {
        Value::String(text) => text,
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    })
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
    use crate::loader::ConfigLoader;
    use crate::secret::Secret;

    #[test]
    fn cfg_028_explain_reports_the_value_and_the_layer_that_supplied_it() {
        let outcome = ConfigLoader::new()
            .with_properties_str("spark.cdm.perfops.numParts  777\n", "cdm.properties")
            .load();

        let explanation = explain("perfops.num_parts", &outcome).unwrap();
        assert_eq!(explanation.canonical, "perfops.num_parts");
        assert_eq!(explanation.legacy, ["spark.cdm.perfops.numParts"]);
        assert_eq!(explanation.kind, "integer");
        assert_eq!(explanation.unit.as_deref(), Some("parts"));
        assert_eq!(explanation.default_value.as_deref(), Some("5000"));
        assert_eq!(explanation.effective_value.as_deref(), Some("777"));
        assert_eq!(explanation.source, Source::File("cdm.properties".into()));
        assert_eq!(explanation.location.as_deref(), Some("cdm.properties:1"));
        assert!(explanation
            .summary
            .starts_with("Number of token-range splits"));

        let rendered = explanation.to_string();
        assert!(rendered.contains("effective: 777"));
        assert!(rendered.contains("cdm.properties:1"));
    }

    #[test]
    fn cfg_028_a_property_nobody_set_is_explained_as_a_default() {
        let outcome = ConfigLoader::new().load();
        let explanation = explain("spark.cdm.perfops.batchSize", &outcome).unwrap();
        assert_eq!(explanation.canonical, "perfops.batch_size");
        assert_eq!(explanation.source, Source::Defaults);
        assert_eq!(explanation.effective_value.as_deref(), Some("5"));
        assert!(explanation.location.is_none());
        assert!(explanation.to_string().contains("built-in default"));
    }

    #[test]
    fn cfg_028_explaining_a_secret_never_prints_it() {
        let outcome = ConfigLoader::new()
            .with_overrides(["connect.origin.password=hunter2"])
            .load();
        let explanation = explain("connect.origin.password", &outcome).unwrap();
        assert_eq!(explanation.effective_value.as_deref(), Some("***"));
        assert_eq!(explanation.default_value.as_deref(), Some("***"));
        // Not just the value: the provenance string must not carry it either, which is where an
        // earlier revision of the loader leaked `--set connect.origin.password=hunter2` whole.
        assert_eq!(
            explanation.location.as_deref(),
            Some("--set connect.origin.password")
        );
        let rendered = explanation.to_string();
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn cfg_028_an_unknown_key_has_no_explanation() {
        let outcome = ConfigLoader::new().load();
        assert!(explain("perfops.nonesuch", &outcome).is_none());
    }

    #[test]
    fn cfg_029_diff_ignores_ordering_spelling_and_shared_defaults() {
        // The same two settings, written in a different order and in different dialects.
        let a = ConfigLoader::new()
            .with_properties_str(
                "spark.cdm.perfops.numParts 10\nspark.cdm.schema.origin.keyspaceTable ks.t\n",
                "a.properties",
            )
            .load()
            .config
            .unwrap();
        let b = ConfigLoader::new()
            .with_api_body(serde_json::json!({
                "schema": {"origin": {"keyspace_table": "ks.t"}},
                "perfops": {"num_parts": 10},
            }))
            .load()
            .config
            .unwrap();

        let difference = diff(&a, &b);
        assert!(difference.is_empty(), "{difference}");
        assert_eq!(
            difference.to_string(),
            "the two configurations are equivalent"
        );
    }

    #[test]
    fn cfg_029_diff_reports_every_property_that_differs() {
        let mut a = CdmConfig::default();
        a.perfops.num_parts = 10;
        a.schema.origin.column.skip = vec!["x".to_owned()];

        let mut b = CdmConfig::default();
        b.perfops.num_parts = 20;
        b.connect.target.host = "elsewhere".to_owned();

        let difference = diff(&a, &b);
        let changed: Vec<&str> = difference
            .changes
            .iter()
            .map(|c| c.canonical.as_str())
            .collect();
        assert_eq!(
            changed,
            [
                "connect.target.host",
                "schema.origin.column.skip",
                "perfops.num_parts",
            ]
        );

        let parts = &difference.changes[2];
        assert_eq!(parts.left.as_deref(), Some("10"));
        assert_eq!(parts.right.as_deref(), Some("20"));

        let rendered = difference.to_string();
        assert!(rendered.contains("- 10"));
        assert!(rendered.contains("+ 20"));
    }

    #[test]
    fn cfg_029_a_credential_difference_is_visible_but_not_readable() {
        let mut a = CdmConfig::default();
        a.connect.origin.password = Secret::new("one");
        let mut b = CdmConfig::default();
        b.connect.origin.password = Secret::new("two");

        // SEC-001: `Secret` serialises as `***`, so the diff cannot see the difference either.
        // That is the correct trade: a diff that could tell them apart could print them.
        let difference = diff(&a, &b);
        assert!(difference.is_empty());
        assert!(!difference.to_string().contains("one"));
    }
}
