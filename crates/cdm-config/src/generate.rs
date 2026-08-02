//! Generated projections of the model: the JSON Schema and the property table (`CFG-003`).
//!
//! Both are written by `cargo xtask docs`, and CI fails if the checked-in copies differ from
//! what this module produces (`OPS-012`). Nothing here may be hand-edited.

use std::fmt::Write as _;

use serde_json::Value;

use crate::meta::{PropertyKind, PropertyMeta};
use crate::model::CdmConfig;
use crate::registry::PropertyRegistry;

/// The banner that marks a generated file so nobody edits it by hand.
const BANNER: &str = "<!-- GENERATED FILE — run `cargo xtask docs`. Do not edit by hand. -->";

/// The JSON Schema of [`CdmConfig`], as published at `schema/cdm-config.schema.json`
/// (`CFG-003`).
///
/// ```
/// let schema = cdm_config::json_schema();
/// assert_eq!(schema["title"], "CdmConfig");
/// assert_eq!(
///     schema["properties"]["perfops"]["$ref"],
///     "#/$defs/PerfOps"
/// );
/// ```
pub fn json_schema() -> Value {
    let schema = schemars::schema_for!(CdmConfig);
    serde_json::to_value(schema).unwrap_or(Value::Null)
}

/// The JSON Schema, pretty-printed with a trailing newline, exactly as it is checked in.
pub fn json_schema_document() -> String {
    let mut text = serde_json::to_string_pretty(&json_schema()).unwrap_or_default();
    text.push('\n');
    text
}

/// The property reference table published at `docs/generated/PROPERTIES.md`.
///
/// Generated from the same [`PropertyRegistry`] the loaders use, so a property cannot be
/// documented and unimplemented, or implemented and undocumented (`CFG-001`).
pub fn properties_markdown() -> String {
    let registry = PropertyRegistry::global();
    let mut out = String::new();

    out.push_str(BANNER);
    out.push_str("\n\n# cdm-rs — Configuration property reference\n\n");
    let _ = writeln!(
        out,
        "Generated from `cdm_config::CdmConfig` (`CFG-001`, `CFG-002`). `legacy` is the Java \
         `spark.cdm.*` name that cdm-rs still accepts (`CFG-011`); `canonical` is the cdm-rs \
         name used by TOML, YAML, JSON, `CDM__*` environment variables and `--set`.\n"
    );
    let _ = writeln!(out, "{} properties.\n", registry.all().len());

    let mut current_section = String::new();
    for meta in registry.all() {
        let section = section_of(&meta.canonical);
        if section != current_section {
            section.clone_into(&mut current_section);
            let _ = writeln!(out, "\n## `{section}`\n");
            if let Some(note) = section_note(&section) {
                let _ = writeln!(out, "{note}\n");
            }
            out.push_str(
                "| canonical | legacy | type | default | unit | stability | description |\n\
                 |---|---|---|---|---|---|---|\n",
            );
        }
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            meta.canonical,
            code_list(&meta.legacy),
            kind_cell(meta.kind),
            default_cell(meta),
            meta.unit.map_or_else(|| "—".to_owned(), str::to_owned),
            meta.stability,
            meta.summary.replace('|', "\\|"),
        );
    }
    out.push('\n');
    out
}

/// The connection-section preamble (`CFG-042`).
///
/// A raw string, because the escaped-newline form silently carries its source indentation into the
/// rendered markdown.
const CONNECT_NOTE: &str = r"Origin and target are configured **independently** (`CON-001`), and each side uses **either** a
contact point **or** an Astra secure-connect-bundle — never both (`CFG-041`).

| Origin → Target | Origin | Target |
|---|---|---|
| Cassandra → Cassandra | `connect.origin.host` | `connect.target.host` |
| DSE → HCD | `connect.origin.host` | `connect.target.host` |
| Cassandra/DSE → Astra | `connect.origin.host` | `connect.target.scb` or `connect.target.astra.database_id` |
| Astra → Astra | `connect.origin.scb` | `connect.target.scb` |
| Astra → Cassandra | `connect.origin.scb` | `connect.target.host` |

**`connect.{side}.scb` and every `connect.{side}.astra.*` property apply to Astra DB only.** They
are ignored for self-managed Apache Cassandra, DSE, HCD and ScyllaDB, which use
`connect.{side}.host` and `connect.{side}.port`.

**TLS to a self-managed cluster is not a bundle.** A cluster with client encryption uses
`connect.{side}.tls.*` — truststore, keystore and cipher suites (`CFG-120`). That is a separate
mechanism, unrelated to the Astra bundle. The one exception is `connect.{side}.tls.is_astra`, a
Java compatibility path that synthesises a bundle from truststore material; new configurations
should not use it.";

/// Prose that a table of rows cannot convey, emitted above a section (`CFG-042`).
///
/// The connection section is the one place where a reader can reasonably misread the properties as
/// a menu to pick from rather than two mutually exclusive styles, so it gets a note.
fn section_note(section: &str) -> Option<&'static str> {
    match section {
        "connect" => Some(CONNECT_NOTE),
        _ => None,
    }
}

/// The top-level section a canonical name belongs to.
fn section_of(canonical: &str) -> String {
    canonical
        .split_once('.')
        .map_or_else(|| canonical.to_owned(), |(head, _)| head.to_owned())
}

/// Legacy aliases as inline code, or an em dash when a property is new to cdm-rs.
fn code_list(aliases: &[String]) -> String {
    if aliases.is_empty() {
        return "—".to_owned();
    }
    aliases
        .iter()
        .map(|alias| format!("`{alias}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The type cell, spelling out an enumeration's variants.
fn kind_cell(kind: PropertyKind) -> String {
    match kind {
        PropertyKind::Enum(variants) => variants
            .iter()
            .map(|variant| format!("`{variant}`"))
            .collect::<Vec<_>>()
            .join(" \\| "),
        other => other.as_str().to_owned(),
    }
}

/// Compares a checked-in artefact with the generated one, ignoring line-ending policy.
///
/// The artefacts are stored with LF, but a Windows checkout may materialise them with CRLF, and
/// the difference is git's business rather than a staleness signal (`OPS-012`).
pub fn is_current(checked_in: &str, generated: &str) -> bool {
    checked_in.replace("\r\n", "\n") == generated.replace("\r\n", "\n")
}

/// The default cell: the value, the note describing a computed default, or an em dash.
fn default_cell(meta: &PropertyMeta) -> String {
    match (meta.displayed_default(), meta.default_note) {
        (Some(value), _) => format!("`{value}`"),
        (None, Some(note)) => note.to_owned(),
        (None, None) => "—".to_owned(),
    }
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

    /// The workspace root, two levels above this crate.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn cfg_003_the_schema_describes_the_whole_model() {
        let schema = json_schema();
        assert_eq!(schema["title"], "CdmConfig");
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );

        let defs = schema["$defs"].as_object().unwrap();
        for section in ["Connect", "SideConnect", "PerfOps", "Feature", "Logging"] {
            assert!(defs.contains_key(section), "{section} is missing");
        }

        // A secret is a password-formatted string, never an object.
        let password = &defs["SideConnect"]["properties"]["password"];
        assert_eq!(password["$ref"], "#/$defs/Secret");
        assert_eq!(defs["Secret"]["format"], "password");

        // A duration is a string, not the two-field struct `std::time::Duration` serialises to.
        assert_eq!(defs["Duration"]["type"], "string");
        assert_eq!(defs["Duration"]["format"], "duration");

        // Enumerations carry their variants, which is what drives the UI's select boxes.
        let levels = defs["ConsistencyLevel"]["oneOf"].as_array().unwrap();
        assert_eq!(levels.len(), 11);
    }

    #[test]
    fn cfg_003_the_checked_in_schema_is_the_generated_one() {
        let path = repo_root().join("schema/cdm-config.schema.json");
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} is missing ({e}); run `cargo xtask docs`",
                path.display()
            )
        });
        assert!(
            is_current(&checked_in, &json_schema_document()),
            "schema/cdm-config.schema.json is stale; run `cargo xtask docs`"
        );
    }

    /// `CFG-042`: the reference must say plainly that the bundle is Astra-only.
    ///
    /// A row in a table cannot carry this, and it is the exact ambiguity that made the README's
    /// quickstart read as though every migration needs a bundle.
    #[test]
    fn cfg_042_the_reference_scopes_the_bundle_to_astra() {
        let markdown = properties_markdown();

        assert!(
            markdown.contains("apply to Astra DB only"),
            "the connect section must scope `scb` and `astra.*` to Astra DB"
        );
        assert!(
            markdown.contains("TLS to a self-managed cluster is not a bundle"),
            "TLS and the bundle are separate mechanisms and must not be conflated"
        );
        assert!(
            markdown.contains(
                "| Cassandra → Cassandra | `connect.origin.host` | `connect.target.host` |"
            ),
            "the self-managed case must be visibly bundle-free"
        );
        assert!(
            markdown.contains("never both (`CFG-041`)"),
            "the note must point at the rule that enforces it"
        );
    }

    /// The preamble must not carry its source indentation into the rendered markdown.
    #[test]
    fn cfg_042_the_section_note_is_not_indented() {
        for line in CONNECT_NOTE.lines() {
            assert!(
                !line.starts_with(' '),
                "indented line would render as a code block: {line:?}"
            );
        }
    }

    #[test]
    fn cfg_002_the_generated_property_table_is_the_checked_in_one() {
        let path = repo_root().join("docs/generated/PROPERTIES.md");
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} is missing ({e}); run `cargo xtask docs`",
                path.display()
            )
        });
        assert!(
            is_current(&checked_in, &properties_markdown()),
            "docs/generated/PROPERTIES.md is stale; run `cargo xtask docs`"
        );
    }

    #[test]
    fn cfg_002_the_property_table_shows_every_property_without_leaking_a_secret() {
        let markdown = properties_markdown();
        let registry = PropertyRegistry::global();
        for meta in registry.all() {
            assert!(
                markdown.contains(&format!("| `{}` |", meta.canonical)),
                "{} is missing from the table",
                meta.canonical
            );
        }
        assert!(markdown.contains("`spark.cdm.perfops.numParts`"));
        assert!(markdown.contains(BANNER));
        // SEC-001: the default password is `cassandra`, and the table shows `***`.
        assert!(markdown.contains("| `connect.origin.password` |"));
        // No secret's built-in default reaches the page in the clear, on either side.
        for meta in registry.secrets() {
            let row = markdown
                .lines()
                .find(|line| line.starts_with(&format!("| `{}` |", meta.canonical)))
                .unwrap();
            if let Some(default) = &meta.default_value {
                assert!(!row.contains(default.as_str()), "{row}");
            }
        }
        let password_row = markdown
            .lines()
            .find(|line| line.starts_with("| `connect.origin.password` |"))
            .unwrap();
        assert!(password_row.contains("`***`"), "{password_row}");
        assert!(!password_row.contains("cassandra"), "{password_row}");
    }

    #[test]
    fn cfg_002_computed_defaults_are_described_rather_than_invented() {
        let markdown = properties_markdown();
        let workers = markdown
            .lines()
            .find(|line| line.starts_with("| `perfops.workers` |"))
            .unwrap();
        assert!(workers.contains("num_cpus"), "{workers}");
        let target = markdown
            .lines()
            .find(|line| line.starts_with("| `schema.target.keyspace_table` |"))
            .unwrap();
        assert!(target.contains("origin keyspace and table"), "{target}");
    }
}
