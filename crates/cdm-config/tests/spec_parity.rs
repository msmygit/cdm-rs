//! The property registry is checked, row by row, against `docs/SPEC.md` §3.5.
//!
//! §3.5 calls itself "the **normative parity list**", generated from Java CDM's `KnownProperties`
//! enum plus `cdm-detailed.properties`. A missing or misnamed property is therefore a parity bug,
//! not a documentation bug, and this file is the gate that catches one.
//!
//! The check is deliberately *derived* rather than restated: it parses the specification at test
//! time instead of holding a second copy of the table, because a hand-maintained copy is exactly
//! what `CFG-001` forbids.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};

use cdm_config::PropertyRegistry;

/// Canonical names where cdm-rs deliberately departs from the name §3.5 prints, and why.
///
/// There is exactly one, and it is forced: §3.5.8 lists `transform.codecs` as a **list** and
/// `transform.codecs.timestamp_format` / `transform.codecs.timestamp_zone` as scalars beneath it.
/// In Java those are three unrelated flat string keys, so nothing collides; in a struct tree
/// `transform.codecs` cannot be both a list and an object. The two scalars are therefore spelled
/// with an underscore. Both legacy `spark.cdm.*` aliases are unaffected, so no existing Java
/// configuration is disturbed.
const CANONICAL_EXCEPTIONS: &[(&str, &str)] = &[
    (
        "transform.codecs.timestamp_format",
        "transform.codec_timestamp_format",
    ),
    (
        "transform.codecs.timestamp_zone",
        "transform.codec_timestamp_zone",
    ),
];

/// One row of a §3.5 table, with `{side}` already expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpecRow {
    section: String,
    legacy: Option<String>,
    canonical: String,
    default_cell: String,
}

fn spec_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("docs/SPEC.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Parses every `#### 3.5.x` table into rows, expanding the `{side}` placeholder.
fn spec_rows() -> Vec<SpecRow> {
    let text = spec_text();
    let mut rows = Vec::new();
    let mut section = String::new();
    let mut in_table = false;
    // §3.5.11 has no `legacy` column: its properties are new to cdm-rs.
    let mut canonical_first = false;

    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("#### 3.5.") {
            section = format!("3.5.{heading}");
            canonical_first = heading.starts_with("11 ");
            in_table = false;
            continue;
        }
        if section.is_empty() {
            continue;
        }
        // Any other heading ends §3.5.
        if line.starts_with("## ") || line.starts_with("### ") {
            section.clear();
            continue;
        }
        if !line.starts_with('|') {
            in_table = false;
            continue;
        }

        let cells: Vec<String> = line
            .trim_matches('|')
            // A cell may contain an escaped pipe, as the enum cells do.
            .replace("\\|", "\u{1}")
            .split('|')
            .map(|cell| cell.trim().replace('\u{1}', "|"))
            .collect();

        // The header row, then the `|---|` separator, then the body.
        if cells
            .first()
            .is_some_and(|c| c == "legacy" || c == "canonical")
        {
            in_table = true;
            continue;
        }
        if !in_table || cells.iter().all(|cell| cell.chars().all(|c| c == '-')) {
            continue;
        }

        let (legacy, canonical, default_cell) = if canonical_first {
            (None, cells[0].clone(), cells[2].clone())
        } else {
            let legacy = strip_code(&cells[0]);
            (legacy, cells[1].clone(), cells[3].clone())
        };
        let Some(canonical) = strip_code(&canonical) else {
            continue;
        };

        for side in ["origin", "target"] {
            let expanded = SpecRow {
                section: section.clone(),
                legacy: legacy.as_ref().map(|alias| alias.replace("{side}", side)),
                canonical: canonical.replace("{side}", side),
                default_cell: default_cell.clone(),
            };
            let templated = canonical.contains("{side}");
            rows.push(expanded);
            if !templated {
                break;
            }
        }
    }
    rows
}

/// The content of a `` `code` `` cell, or `None` for the em dash that means "no such name".
fn strip_code(cell: &str) -> Option<String> {
    let trimmed = cell.trim();
    if trimmed.starts_with('—') || trimmed.is_empty() {
        return None;
    }
    let inner = trimmed
        .split('`')
        .nth(1)
        .map_or_else(|| trimmed.to_owned(), str::to_owned);
    (!inner.is_empty()).then_some(inner)
}

/// The canonical name the registry uses for a name §3.5 prints.
fn expected_canonical(spec_name: &str) -> String {
    CANONICAL_EXCEPTIONS
        .iter()
        .find(|(from, _)| *from == spec_name)
        .map_or_else(|| spec_name.to_owned(), |(_, to)| (*to).to_owned())
}

/// The parity list is not empty and the parser actually understood it.
#[test]
fn cfg_100_the_specification_parity_table_parses() {
    let rows = spec_rows();
    assert!(
        rows.len() > 90,
        "only {} rows parsed out of SPEC §3.5; the parser is broken, not the model",
        rows.len()
    );
    assert!(rows.iter().any(|r| r.canonical == "connect.origin.host"));
    assert!(rows
        .iter()
        .any(|r| r.canonical == "connect.target.tls.is_astra"));
    assert!(rows.iter().any(|r| r.canonical == "logging.diff_file"));
    // `{side}` rows expanded into both sides.
    assert!(rows
        .iter()
        .any(|r| r.canonical == "connect.origin.astra.mode"));
    assert!(rows
        .iter()
        .any(|r| r.canonical == "connect.target.astra.mode"));
}

/// Every property `SPEC` §3.5 lists exists in the registry, under the canonical name it gives.
#[test]
fn cfg_100_every_specified_property_exists_with_its_canonical_name() {
    let registry = PropertyRegistry::global();
    let mut missing = Vec::new();
    for row in spec_rows() {
        let expected = expected_canonical(&row.canonical);
        if registry.by_canonical(&expected).is_none() {
            missing.push(format!(
                "{} — {} ({})",
                row.section, expected, row.canonical
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "{} property/properties in SPEC §3.5 are not implemented:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Every legacy `spark.cdm.*` name `SPEC` §3.5 lists is accepted, and maps to the right property.
#[test]
fn cfg_110_every_specified_legacy_alias_is_accepted_and_maps_correctly() {
    let registry = PropertyRegistry::global();
    let mut problems = Vec::new();
    for row in spec_rows() {
        let Some(legacy) = &row.legacy else { continue };
        let expected = expected_canonical(&row.canonical);
        match registry.by_legacy(legacy) {
            None => problems.push(format!("{legacy} is not accepted (expected → {expected})")),
            Some(meta) if meta.canonical != expected => problems.push(format!(
                "{legacy} maps to {} but SPEC §3.5 says {expected}",
                meta.canonical
            )),
            Some(_) => {}
        }
    }
    assert!(
        problems.is_empty(),
        "{} legacy alias problem(s):\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

/// The registry invents nothing: every property in it appears in `SPEC` §3.5.
///
/// This is the direction that catches a property added to the model without a specification
/// change, which `AGENTS.md` forbids outright.
#[test]
fn cfg_200_the_registry_contains_nothing_the_specification_does_not_list() {
    let specified: BTreeSet<String> = spec_rows()
        .into_iter()
        .map(|row| expected_canonical(&row.canonical))
        .collect();

    let unspecified: Vec<&str> = PropertyRegistry::global()
        .all()
        .iter()
        .map(|meta| meta.canonical.as_str())
        .filter(|canonical| !specified.contains(*canonical))
        .collect();

    assert!(
        unspecified.is_empty(),
        "{} property/properties are implemented but not specified in SPEC §3.5:\n  {}",
        unspecified.len(),
        unspecified.join("\n  ")
    );
}

/// Every default `SPEC` §3.5 states as a plain literal is the default the model applies.
///
/// Cells that describe a default rather than state one — `= origin`, `num_cpus`, `partitioner
/// min`, `true` when server enabled — are checked instead for the presence of a `default_note`,
/// so that a computed default is documented rather than silently absent.
#[test]
fn cfg_160_every_stated_default_matches_the_model() {
    let registry = PropertyRegistry::global();
    let mut problems = Vec::new();
    let mut described = 0_usize;

    for row in spec_rows() {
        let canonical = expected_canonical(&row.canonical);
        let Some(meta) = registry.by_canonical(&canonical) else {
            continue; // reported by cfg_100
        };

        match classify(&row.default_cell) {
            SpecDefault::None => {
                if let Some(actual) = &meta.default_value {
                    problems.push(format!(
                        "{canonical}: SPEC states no default, the model applies `{actual}`"
                    ));
                }
            }
            SpecDefault::Described => {
                // A default the specification describes rather than states must at least be
                // described in the metadata, so that the generated table can say what it is.
                if meta.default_value.is_none() && meta.default_note.is_none() {
                    problems.push(format!(
                        "{canonical}: SPEC says `{}` but the model documents no default at all",
                        row.default_cell
                    ));
                }
                described += 1;
            }
            SpecDefault::Literal(literal) => {
                let actual = meta.default_value.clone().unwrap_or_default();
                if !defaults_agree(&literal, &actual) {
                    problems.push(format!(
                        "{canonical}: SPEC says `{literal}`, the model applies `{actual}`"
                    ));
                }
            }
        }
    }

    assert!(
        problems.is_empty(),
        "{} default(s) disagree with SPEC §3.5:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
    assert!(
        described > 0,
        "the described-default path was never exercised"
    );
}

/// What a §3.5 default cell says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SpecDefault {
    /// An em dash: the property has no default and is absent unless set.
    None,
    /// A phrase describing a default that is computed at run time.
    Described,
    /// A value stated outright.
    Literal(String),
}

/// Phrases §3.5 uses for a default that only exists once something else is known.
const DESCRIBED_DEFAULTS: &[&str] = &[
    "num_cpus",
    "= origin",
    "partitioner min",
    "partitioner max",
    "hostname+pid",
    "when server enabled",
];

/// Classifies a §3.5 default cell.
fn classify(cell: &str) -> SpecDefault {
    let trimmed = cell.trim();
    // "— **required**" is still an em dash: no default, and Tier 1 insists on a value.
    if trimmed.starts_with('—') || trimmed.is_empty() {
        return SpecDefault::None;
    }
    if DESCRIBED_DEFAULTS
        .iter()
        .any(|phrase| trimmed.contains(phrase))
    {
        return SpecDefault::Described;
    }
    // `` `0` (unlimited) `` and `` `sni` (`CON-022`, `CON-026`) `` state a default and then
    // annotate it, so only the first backticked run is the value.
    trimmed.split('`').nth(1).map_or_else(
        || SpecDefault::Described,
        |literal| SpecDefault::Literal(literal.to_owned()),
    )
}

/// Whether a stated default and the model's default mean the same thing.
///
/// Two spellings are reconciled rather than demanded to match byte for byte: durations, because
/// §3.5 writes `60s` where the humantime formatter the model round-trips through prints `1m`;
/// and numbers, because §3.5 writes `0` for the `f64` guardrail the model renders as `0.0`.
fn defaults_agree(spec: &str, actual: &str) -> bool {
    if spec == actual {
        return true;
    }
    if let (Ok(a), Ok(b)) = (spec.parse::<f64>(), actual.parse::<f64>()) {
        return (a - b).abs() < f64::EPSILON;
    }
    match (parse_duration(spec), parse_duration(actual)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The `HhMmSs`/`ms` durations that appear in §3.5, in milliseconds.
fn parse_duration(text: &str) -> Option<u64> {
    let units: BTreeMap<&str, u64> = [("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)]
        .into_iter()
        .collect();
    let mut total = 0_u64;
    let mut number = String::new();
    let mut unit = String::new();
    let mut saw_unit = false;

    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !unit.is_empty() {
                total += number.parse::<u64>().ok()? * units.get(unit.as_str())?;
                number.clear();
                unit.clear();
            }
            number.push(ch);
        } else if ch.is_ascii_alphabetic() {
            saw_unit = true;
            unit.push(ch);
        } else {
            return None;
        }
    }
    if !saw_unit || number.is_empty() {
        return None;
    }
    total += number.parse::<u64>().ok()? * units.get(unit.as_str())?;
    Some(total)
}

/// The accepted consistency levels are exactly those `SPEC` §3.5.7 lists (`CFG-161`).
#[test]
fn cfg_161_the_accepted_consistency_levels_are_exactly_those_specified() {
    let text = spec_text();
    let listed = text
        .split("Accepted consistency levels **[P]**:")
        .nth(1)
        .expect("SPEC §3.5.7 states the accepted consistency levels")
        .split("(case-insensitive)")
        .next()
        .unwrap()
        .replace('`', "")
        .replace('\n', " ");

    let specified: BTreeSet<&str> = listed
        .split(',')
        .map(str::trim)
        .filter(|level| !level.is_empty())
        .collect();

    let implemented: BTreeSet<&str> = cdm_config::types::ConsistencyLevel::VARIANTS
        .iter()
        .copied()
        .collect();

    assert_eq!(
        specified, implemented,
        "the consistency levels in SPEC §3.5.7 and in the model disagree"
    );
}
