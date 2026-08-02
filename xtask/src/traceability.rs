//! The requirement traceability gate (`OPS-011`).
//!
//! Verifies the contract between `docs/SPEC.md`, `docs/TRACEABILITY.md` and the test suite:
//!
//! 1. every requirement ID declared in `SPEC.md` has exactly one row in `TRACEABILITY.md`;
//! 2. every ID in `TRACEABILITY.md` is declared in `SPEC.md` (no phantom requirements);
//! 3. no ID is declared twice in either document;
//! 4. every ID cited by a test exists in `SPEC.md` (no orphaned citations);
//! 5. no matrix row has an unterminated code span, which would silently swallow the rest of the
//!    cell when the document is rendered.
//!
//! The "every done requirement has a citing test" half of `OPS-011` needs a status field to be
//! meaningful. It arrives with `docs/traceability.toml` once the first requirements are actually
//! implemented; until then rule 4 catches the failure mode that can occur today — a test citing an
//! ID that does not exist.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use regex::Regex;

/// Domains recognised by the ID grammar. Keeping this closed catches typos such as `MIGR-001`
/// that an open `[A-Z]+-\d+` pattern would silently accept.
const DOMAINS: &[&str] = &[
    "CFG", "CON", "SCH", "TOK", "ENG", "MIG", "VAL", "GRD", "FEA", "CDC", "TRK", "DST", "MET",
    "API", "MCP", "A2A", "UI", "CLI", "PLG", "ERR", "SEC", "TST", "OPS", "NFR", "COMPAT",
];

/// Run the gate against a workspace root, reporting every violation rather than the first.
pub(crate) fn check(root: &Path) -> anyhow::Result<()> {
    let spec_path = root.join("docs/SPEC.md");
    let matrix_path = root.join("docs/TRACEABILITY.md");

    let spec = read(&spec_path)?;
    let matrix = read(&matrix_path)?;

    let declared = declared_ids(&spec)?;
    let traced = traced_ids(&matrix)?;

    let mut problems: Vec<String> = Vec::new();

    for (id, count) in &declared {
        if *count > 1 {
            problems.push(format!(
                "{id} is declared {count} times in docs/SPEC.md; IDs are unique and append-only"
            ));
        }
    }
    for (id, count) in &traced {
        if *count > 1 {
            problems.push(format!(
                "{id} appears {count} times in docs/TRACEABILITY.md"
            ));
        }
    }

    let declared_set: BTreeSet<&String> = declared.keys().collect();
    let traced_set: BTreeSet<&String> = traced.keys().collect();

    for id in declared_set.difference(&traced_set) {
        problems.push(format!(
            "{id} is specified in docs/SPEC.md but has no row in docs/TRACEABILITY.md"
        ));
    }
    for id in traced_set.difference(&declared_set) {
        problems.push(format!(
            "{id} is traced in docs/TRACEABILITY.md but is not specified in docs/SPEC.md"
        ));
    }

    problems.extend(orphaned_test_citations(root, &declared_set)?);
    problems.extend(unbalanced_code_spans(&matrix));

    if problems.is_empty() {
        println!(
            "traceability: {} requirements specified, {} traced, 0 problems",
            declared.len(),
            traced.len()
        );
        return Ok(());
    }

    problems.sort();
    let listing = problems
        .iter()
        .map(|p| format!("  - {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::bail!(
        "{} traceability problem(s) (OPS-011):\n{listing}",
        problems.len()
    )
}

fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))
}

fn id_alternation() -> String {
    DOMAINS.join("|")
}

/// IDs declared in `SPEC.md`, i.e. written as `**ID**` or `**ID [P]**` at the start of a normative
/// statement. Bare in-text references such as ``(`MIG-012`)`` are cross-references, not
/// declarations, and are deliberately not matched here.
fn declared_ids(spec: &str) -> anyhow::Result<BTreeMap<String, usize>> {
    let re = Regex::new(&format!(
        r"\*\*(?<id>(?:{})-\d{{3}})(?:\s+\[(?:P\+?|N)\])?\*\*",
        id_alternation()
    ))?;
    Ok(tally(re.captures_iter(spec).map(|c| c["id"].to_string())))
}

/// IDs traced in `TRACEABILITY.md`, i.e. appearing in the first cell of a matrix row.
fn traced_ids(matrix: &str) -> anyhow::Result<BTreeMap<String, usize>> {
    let re = Regex::new(&format!(
        r"(?m)^\|\s*`(?<id>(?:{})-\d{{3}})`",
        id_alternation()
    ))?;
    Ok(tally(re.captures_iter(matrix).map(|c| c["id"].to_string())))
}

/// Requirement IDs cited by test names (`mig_012_...`) that do not exist in `SPEC.md`.
fn orphaned_test_citations(
    root: &Path,
    declared: &BTreeSet<&String>,
) -> anyhow::Result<Vec<String>> {
    let re = Regex::new(&format!(
        r"fn\s+(?<lower>(?i:{})_\d{{3}})_",
        id_alternation()
    ))?;

    let mut problems = Vec::new();
    for file in rust_sources(&root.join("crates"))? {
        let text = read(&file)?;
        for caps in re.captures_iter(&text) {
            let id = caps["lower"].to_uppercase().replacen('_', "-", 1);
            if !declared.contains(&id) {
                problems.push(format!(
                    "{} names a test after {id}, which is not specified in docs/SPEC.md",
                    file.strip_prefix(root).unwrap_or(&file).display()
                ));
            }
        }
    }
    Ok(problems)
}

/// Matrix rows whose backticks do not pair up.
///
/// The generator that first produced this file truncated long summaries to a fixed width, which
/// could cut a row in the middle of a code span. An unterminated backtick makes the rest of the
/// cell parse as an HTML tag, so `<pk>` and `<const>` silently vanish when the document is
/// rendered — invisible in a diff, obvious on the published site. Fifteen rows shipped that way.
fn unbalanced_code_spans(matrix: &str) -> Vec<String> {
    matrix
        .lines()
        .filter(|line| line.starts_with("| `"))
        .filter(|line| line.matches('`').count() % 2 != 0)
        .map(|line| {
            let id = line.split('`').nth(1).unwrap_or("<unknown>");
            format!(
                "{id} has an unterminated code span; the rest of the cell will render as an HTML \
                 tag rather than text"
            )
        })
        .collect()
}

fn rust_sources(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn tally(ids: impl Iterator<Item = String>) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for id in ids {
        *map.entry(id).or_insert(0) += 1;
    }
    map
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the
// no-panic rule (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn ops_011_declarations_are_recognised_with_and_without_parity_markers() {
        let spec =
            "**MIG-001 [P]** — a\n**CFG-010 [P+]** — b\n**API-003 [N]** — c\n**OPS-011** — d";
        let ids = declared_ids(spec).unwrap();
        assert_eq!(ids.len(), 4);
        assert!(ids.contains_key("MIG-001"));
        assert!(ids.contains_key("OPS-011"));
    }

    #[test]
    fn ops_011_cross_references_are_not_mistaken_for_declarations() {
        let spec = "**MIG-001 [P]** — see `MIG-002` and (`CFG-010`) for detail.";
        let ids = declared_ids(spec).unwrap();
        assert_eq!(ids.keys().collect::<Vec<_>>(), vec!["MIG-001"]);
    }

    #[test]
    fn ops_011_duplicate_declarations_are_counted() {
        let ids = declared_ids("**MIG-001 [P]** — a\n**MIG-001 [P]** — b").unwrap();
        assert_eq!(ids["MIG-001"], 2);
    }

    #[test]
    fn ops_011_matrix_rows_are_recognised() {
        let matrix = "| `MIG-001` <sup>P</sup> | text | `cdm-engine` | `mig_001_*` | #21 |\n\
                      | `CFG-010` | text | `cdm-config` | `cfg_010_*` | #5 |";
        let ids = traced_ids(matrix).unwrap();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn ops_011_unknown_domains_are_rejected_by_the_grammar() {
        // `MIGR` is not a domain; a typo like this must not be silently accepted.
        assert!(declared_ids("**MIGR-001 [P]** — a").unwrap().is_empty());
    }

    #[test]
    fn ops_011_an_unterminated_code_span_is_reported() {
        let matrix =
            "| `MIG-030` | uses `SET c = c + ?`, then `<target… | `cdm-engine` | x | #22 |";
        let problems = unbalanced_code_spans(matrix);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("MIG-030"), "{problems:?}");
    }

    #[test]
    fn ops_011_balanced_rows_are_accepted() {
        let matrix = "| `MIG-030` | uses `SET c = c + ?` on the target | `cdm-engine` | x | #22 |";
        assert!(unbalanced_code_spans(matrix).is_empty());
    }

    #[test]
    fn ops_011_the_repository_itself_passes_the_gate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        check(root).expect("docs/SPEC.md and docs/TRACEABILITY.md must agree");
    }
}
