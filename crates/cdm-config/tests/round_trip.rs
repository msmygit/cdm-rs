//! The configuration round-trip property (`TST-010`, `CFG-001`, `CFG-010`).
//!
//! # The property
//!
//! *Parse, serialise, parse again, and nothing moves.* A configuration is read from an untyped
//! source — a Java `.properties` file, a `--set` override, an environment variable — coerced by
//! [`PropertyKind`], deserialised into [`CdmConfig`], and later written back out: into
//! `cdm config explain`, into the run's record of what it was asked to do, into the JSON body the
//! HTTP control plane echoes. If that trip is not the identity, two runs configured "the same way"
//! are not configured the same way, and the difference surfaces as data.
//!
//! # The one documented exception, and why it is not a leak in this property
//!
//! `SEC-001` requires a secret to serialise as `***`. So a secret genuinely does *not* round-trip,
//! by design, and [`sec_001_a_secret_deliberately_does_not_round_trip`] pins that rather than
//! letting the generator quietly find it and call it a defect. Secret-kind properties are excluded
//! from the generator for the same reason, and the exclusion is asserted to be exactly the secret
//! ones — a property test that silently skipped half the model would be worse than none.
//!
//! # Why generated rather than hand-written
//!
//! The registry has several hundred properties across fifteen kinds, and the interesting failures
//! are per *kind*: a duration that re-renders as `30000ms` instead of `30s`, an enum whose
//! serialised spelling differs from the one the loader accepts, a `bigint` that cannot survive a
//! JSON number. A hand-written case covers the property somebody was thinking about; a generator
//! covers the one they were not. Every value is drawn from a seeded, shrinking `proptest`
//! generator, so a counterexample names the property that broke.

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cdm_config::meta::{PropertyKind, PropertyMeta};
use cdm_config::{CdmConfig, ConfigLoader, PropertyRegistry};
use proptest::prelude::*;
use serde_json::Value;

/// Values that are legal for `kind`, and unlike the defaults wherever that is safe.
///
/// One list per kind rather than one per property: the round trip is a property of the *type*,
/// and a per-property table would be a second copy of the model to keep in step.
fn candidates(kind: &PropertyKind) -> Vec<String> {
    match kind {
        PropertyKind::Bool => vec!["true".to_owned(), "false".to_owned()],
        // Kept inside `0..=100` so that one list serves every width in the model: several
        // properties are `u8` percentages, and a value a `u8` cannot hold would fail to
        // deserialise for a reason that has nothing to do with round-tripping.
        PropertyKind::Integer => vec![
            "0".to_owned(),
            "1".to_owned(),
            "7".to_owned(),
            "100".to_owned(),
        ],
        PropertyKind::BigInteger => vec![
            "0".to_owned(),
            "-9223372036854775808".to_owned(),
            "170141183460469231731687303715884105727".to_owned(),
        ],
        PropertyKind::Float => vec!["0".to_owned(), "0.5".to_owned(), "1".to_owned()],
        PropertyKind::String => vec![
            "value".to_owned(),
            String::new(),
            "a value with spaces".to_owned(),
            "ünïcødé".to_owned(),
        ],
        PropertyKind::Path => vec!["/tmp/cdm".to_owned(), "relative/path.log".to_owned()],
        PropertyKind::Secret => Vec::new(),
        PropertyKind::List => vec!["a".to_owned(), "a,b".to_owned(), "a, b , c".to_owned()],
        PropertyKind::Duration => vec![
            "30s".to_owned(),
            "1500ms".to_owned(),
            "2m".to_owned(),
            "1h".to_owned(),
        ],
        PropertyKind::Enum(variants) => variants.iter().map(|v| (*v).to_owned()).collect(),
        PropertyKind::Socket => vec!["127.0.0.1:8080".to_owned(), "0.0.0.0:9042".to_owned()],
        PropertyKind::Uuid => vec![
            "00000000-0000-0000-0000-000000000000".to_owned(),
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_owned(),
        ],
        PropertyKind::Regex => vec![",".to_owned(), "[a-z]+".to_owned()],
        PropertyKind::TimeZone => vec!["UTC".to_owned(), "Europe/London".to_owned()],
        PropertyKind::Url => vec![
            "https://example.invalid/".to_owned(),
            "http://127.0.0.1:1234/v1".to_owned(),
        ],
    }
}

/// Every property a generated configuration may set: everything but the secrets.
fn assignable() -> Vec<&'static PropertyMeta> {
    PropertyRegistry::global()
        .all()
        .iter()
        .filter(|meta| !meta.secret && !candidates(&meta.kind).is_empty())
        .collect()
}

/// The value at a dotted canonical path, or `None` if the path does not exist.
fn at_path<'a>(tree: &'a Value, canonical: &str) -> Option<&'a Value> {
    canonical
        .split('.')
        .try_fold(tree, |node, segment| node.get(segment))
}

/// Loads a configuration from `--set`-style overrides, which is the shortest untyped path in.
fn load(overrides: &[String]) -> CdmConfig {
    let outcome = ConfigLoader::new().with_overrides(overrides).load();
    assert!(
        !outcome.has_errors(),
        "the generated overrides did not load: {:?}",
        outcome.diagnostics
    );
    outcome
        .config
        .expect("a load without errors yields a config")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `TST-010`, `CFG-001`: parse → serialise → parse is the identity.
    ///
    /// The generated assignment is loaded, the resulting model is serialised to JSON, and the JSON
    /// is deserialised back. The two models must render identically: a value that changed shape on
    /// the way through — a duration re-spelled, an enum re-cased, a `bigint` narrowed to a JSON
    /// number — shows up here as a difference, with the property named.
    #[test]
    fn tst_010_cfg_001_a_configuration_survives_being_written_out_and_read_back(
        selection in proptest::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>()), 1..24),
    ) {
        let properties = assignable();
        let mut overrides = Vec::new();
        let mut chosen = Vec::new();
        for (property, value) in selection {
            let meta = property.get(&properties);
            let options = candidates(&meta.kind);
            let raw = value.get(&options).clone();
            overrides.push(format!("{}={raw}", meta.canonical));
            chosen.push(meta.canonical.clone());
        }

        let first = load(&overrides);
        let written = serde_json::to_value(&first).unwrap();
        let second: CdmConfig = serde_json::from_value(written.clone()).unwrap();
        let rewritten = serde_json::to_value(&second).unwrap();

        prop_assert_eq!(&written, &rewritten, "the model did not survive the round trip");

        // Not vacuous: every property the generator named must exist at the canonical path it is
        // registered under. A `#[serde(rename)]` that drifted from the registry would make the
        // equality above hold while `--set` silently set nothing.
        for canonical in &chosen {
            prop_assert!(
                at_path(&written, canonical).is_some(),
                "`{}` is in the registry but not at that path in the serialised model",
                canonical,
            );
        }
    }

    /// `TST-010`, `CFG-010`: a value set through an untyped source is the value the model holds.
    ///
    /// The other half of the round trip, and the half a fixed-point assertion cannot see: a loader
    /// that dropped every override would still round-trip perfectly, because the defaults do.
    #[test]
    fn tst_010_cfg_010_every_assigned_value_reaches_the_model_it_was_assigned_to(
        property in any::<prop::sample::Index>(),
        value in any::<prop::sample::Index>(),
    ) {
        let properties = assignable();
        let meta = property.get(&properties);
        let options = candidates(&meta.kind);
        let raw = value.get(&options).clone();

        let expected = meta.kind.coerce(&raw).unwrap();
        let config = load(&[format!("{}={raw}", meta.canonical)]);
        let written = serde_json::to_value(&config).unwrap();
        let actual = at_path(&written, &meta.canonical).unwrap();

        // Two kinds are deliberately re-rendered rather than echoed, and both are documented
        // behaviour rather than a round-trip failure: a duration is normalised by `humantime`,
        // and a `bigint` is carried as a string on the way in and as its own type on the way out.
        // For everything else the coerced value and the serialised one are the same JSON.
        if !matches!(meta.kind, PropertyKind::Duration | PropertyKind::BigInteger) {
            prop_assert_eq!(
                actual,
                &expected,
                "`{}` was set to `{}` and the model does not hold it",
                meta.canonical,
                raw,
            );
        }

        // Whatever the rendering, the value has to survive a second trip unchanged.
        let again: CdmConfig = serde_json::from_value(written.clone()).unwrap();
        prop_assert_eq!(
            at_path(&serde_json::to_value(&again).unwrap(), &meta.canonical).cloned(),
            Some(actual.clone()),
        );
    }
}

#[test]
fn sec_001_a_secret_deliberately_does_not_round_trip() {
    // The one exception to the property above, pinned so that it is a decision rather than a gap.
    // A serialised configuration is something an operator pastes into a ticket; `SEC-001` is worth
    // more than the round trip, and the loader never re-serialises in order to reload.
    let config = load(&["connect.origin.password=hunter2".to_owned()]);
    assert_eq!(config.connect.origin.password.expose(), "hunter2");

    let written = serde_json::to_value(&config).unwrap();
    assert_eq!(
        at_path(&written, "connect.origin.password").unwrap(),
        &Value::String("***".to_owned()),
        "a secret must never be serialised"
    );

    let reloaded: CdmConfig = serde_json::from_value(written).unwrap();
    assert_eq!(
        reloaded.connect.origin.password.expose(),
        "***",
        "and the redaction is what comes back, which is why this path is not a reload path"
    );
}

#[test]
fn tst_010_the_generator_skips_exactly_the_secret_properties() {
    // A property test over a filtered model is only as good as its filter. If a kind were ever
    // added without candidate values, it would drop silently out of the generator and the round
    // trip would stop covering it.
    let skipped: Vec<&str> = PropertyRegistry::global()
        .all()
        .iter()
        .filter(|meta| !meta.secret && candidates(&meta.kind).is_empty())
        .map(|meta| meta.canonical.as_str())
        .collect();
    assert!(
        skipped.is_empty(),
        "these non-secret properties have no generated values: {skipped:?}"
    );
    assert!(
        !assignable().is_empty(),
        "the generator must have something to draw from"
    );
    assert!(
        PropertyRegistry::global().secrets().count() > 0,
        "and the exclusion must exclude something"
    );
}
