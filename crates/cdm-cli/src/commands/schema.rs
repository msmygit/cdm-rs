//! `cdm schema show` and `cdm schema diff` (`SCH-008`).
//!
//! `diff` is the command to run immediately before a migration. It resolves the *same*
//! [`ColumnMapping`](cdm_cql::statement::ColumnMapping) the run will resolve, plans the *same*
//! conversions (`CDC-010`), and prints them — so an operator finds out that a `text` column is
//! landing in a `timestamp` with no codec enabled now, rather than from a wrong value in production
//! six hours in.
//!
//! It goes through the harness's introspection step rather than reimplementing it, which is what
//! makes "the same" true rather than aspirational: a mapping this command accepts is a mapping the
//! run accepts, because it is the one the run builds.

use std::io::Write;

use cdm_codec::{ConversionPlan, CqlTypeInfo};
use cdm_core::{CdmError, Diagnostic};
use cdm_cql::schema::table::TableSchema;
use cdm_cql::statement::TargetSource;
use serde::Serialize;

use crate::cli::{ConfigArgs, JobArgs};
use crate::harness;
use crate::output::{render_diagnostics, Report};

/// One table's schema, as `cdm schema show` prints it (`SCH-001`).
#[derive(Debug, Serialize)]
pub struct TableReport {
    /// The `keyspace.table` reference.
    pub table: String,
    /// Every column, keys first.
    pub columns: Vec<ColumnReport>,
    /// Whether every non-key column is a counter (`SCH-005`).
    pub counter_table: bool,
}

/// One column.
#[derive(Debug, Serialize)]
pub struct ColumnReport {
    /// The column's name, internal form.
    pub name: String,
    /// The CQL type, exactly as `system_schema` spells it.
    pub cql_type: String,
    /// `partition_key`, `clustering`, `static` or `regular`.
    pub kind: String,
}

/// What `cdm schema show` reports.
#[derive(Debug, Serialize)]
pub struct ShowReport {
    /// The origin table.
    pub origin: TableReport,
    /// The target table.
    pub target: TableReport,
}

impl Report for ShowReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        render_table(&self.origin, "origin", out)?;
        writeln!(out)?;
        render_table(&self.target, "target", out)
    }
}

fn render_table(table: &TableReport, label: &str, out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "{label}: {}", table.table)?;
    if table.counter_table {
        writeln!(
            out,
            "  (counter table — writes are not idempotent, MIG-032)"
        )?;
    }
    let width = table
        .columns
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0);
    for column in &table.columns {
        writeln!(
            out,
            "  {:width$}  {}  [{}]",
            column.name, column.cql_type, column.kind
        )?;
    }
    Ok(())
}

/// What `cdm schema diff` reports (`SCH-008`).
#[derive(Debug, Serialize)]
pub struct DiffReport {
    /// The origin table.
    pub origin_table: String,
    /// The target table.
    pub target_table: String,
    /// One row per target column, in target order.
    pub columns: Vec<MappedColumn>,
    /// Origin columns that reach no target column, which is data being left behind.
    pub origin_only: Vec<String>,
    /// Anything the conversion planner warned about (`CDC-016`).
    pub incompatibilities: Vec<Diagnostic>,
}

/// One target column and where its value comes from (`SCH-003`, `CDC-010`).
#[derive(Debug, Serialize)]
pub struct MappedColumn {
    /// The target column.
    pub target: String,
    /// Its CQL type on the target.
    pub target_type: String,
    /// The origin column feeding it, or the feature that does.
    pub origin: String,
    /// The origin's CQL type, when an origin column supplies it.
    pub origin_type: Option<String>,
    /// How the value is converted: `passthrough`, `codec`, `list`, `unsupported`, …
    pub conversion: String,
}

impl Report for DiffReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "{} → {}\n", self.origin_table, self.target_table)?;

        let width = |f: fn(&MappedColumn) -> &str| {
            self.columns
                .iter()
                .map(|c| f(c).chars().count())
                .max()
                .unwrap_or(0)
        };
        let origin_width = width(|c| c.origin.as_str());
        let target_width = width(|c| c.target.as_str());

        for column in &self.columns {
            writeln!(
                out,
                "  {:origin_width$} {:>14}  ->  {:target_width$} {:<14}  [{}]",
                column.origin,
                column.origin_type.as_deref().unwrap_or("—"),
                column.target,
                column.target_type,
                column.conversion
            )?;
        }

        if !self.origin_only.is_empty() {
            // Not an error — `schema.origin.column.skip` and a narrower target are both legitimate
            // — but it is the thing an operator most often did not mean to do.
            writeln!(
                out,
                "\n{} origin column(s) reach no target column and will not be migrated:\n  {}",
                self.origin_only.len(),
                self.origin_only.join(", ")
            )?;
        }

        if self.incompatibilities.is_empty() {
            writeln!(out, "\nNo incompatibilities found.")
        } else {
            writeln!(out)?;
            render_diagnostics(&self.incompatibilities, out)
        }
    }

    fn has_findings(&self) -> bool {
        !self.incompatibilities.is_empty()
    }
}

/// Prints both tables' schemas (`SCH-001`, `SCH-008`).
///
/// # Errors
///
/// As the harness's introspection step: an unusable configuration, an unreachable cluster, or a
/// table that does not exist.
pub fn show(args: &ConfigArgs) -> Result<ShowReport, CdmError> {
    let resolved = harness::resolve_tables(&job_args(args))?;
    Ok(ShowReport {
        origin: describe(&resolved.origin),
        target: describe(&resolved.target),
    })
}

/// Compares the two schemas, with the mapping and the conversion plan (`SCH-008`).
///
/// # Errors
///
/// As [`show`], plus [`cdm_core::ErrorKind::SchemaMismatch`] for a mapping that cannot be resolved
/// at all — a target primary-key component nothing supplies, say. That is a failure rather than a
/// finding: there is no diff to print for a pairing that cannot be executed.
pub fn diff(args: &ConfigArgs) -> Result<DiffReport, CdmError> {
    let resolved = harness::resolve_tables(&job_args(args))?;
    let planner = cdm_codec::Planner::new(
        cdm_codec::CodecRegistry::with_builtins(&[], None)?,
        cdm_codec::PlannerOptions::default(),
    );

    let mapping = &resolved.mapping;
    let mut columns = Vec::with_capacity(mapping.target_columns().len());
    let mut incompatibilities = Vec::new();
    let mut consumed = vec![false; mapping.origin_columns().len()];

    for (index, target) in mapping.target_columns().iter().enumerate() {
        let source = mapping.source(index);
        let origin_column = match source {
            Some(TargetSource::Origin(origin_index) | TargetSource::ExtractJson(origin_index)) => {
                if let Some(slot) = consumed.get_mut(*origin_index) {
                    *slot = true;
                }
                mapping.origin_columns().get(*origin_index)
            }
            _ => None,
        };

        let (origin_name, origin_type) = match (source, origin_column) {
            (_, Some(column)) => (column.name.clone(), Some(column.cql_type.clone())),
            (Some(TargetSource::Constant(literal)), _) => (format!("(constant {literal})"), None),
            (Some(TargetSource::ExplodeKey), _) => ("(explode key)".to_owned(), None),
            (Some(TargetSource::ExplodeValue), _) => ("(explode value)".to_owned(), None),
            _ => ("(unset)".to_owned(), None),
        };

        // A column with no origin has nothing to convert; saying `passthrough` would imply bytes
        // move that do not.
        let conversion = match &origin_type {
            None => "—".to_owned(),
            Some(from) => {
                let plan = planner.plan_column(
                    &target.name,
                    &parse_type(from),
                    &parse_type(&target.cql_type),
                );
                incompatibilities.extend(plan.diagnostics().iter().cloned());
                kind_of(plan.plan()).to_owned()
            }
        };

        columns.push(MappedColumn {
            target: target.name.clone(),
            target_type: target.cql_type.clone(),
            origin: origin_name,
            origin_type,
            conversion,
        });
    }

    let origin_only = mapping
        .origin_columns()
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed.get(*index).copied().unwrap_or(false))
        .map(|(_, column)| column.name.clone())
        .collect();

    Ok(DiffReport {
        origin_table: resolved.origin.quoted_name(),
        target_table: resolved.target.quoted_name(),
        columns,
        origin_only,
        incompatibilities,
    })
}

/// The name a conversion plan goes by, for the `[…]` column.
const fn kind_of(plan: &ConversionPlan) -> &'static str {
    match plan {
        ConversionPlan::Passthrough => "passthrough",
        ConversionPlan::Codec(_) => "codec",
        ConversionPlan::Udt { .. } => "udt",
        ConversionPlan::List(_) => "list",
        ConversionPlan::Set(_) => "set",
        ConversionPlan::Map { .. } => "map",
        ConversionPlan::Tuple(_) => "tuple",
        ConversionPlan::Vector { .. } => "vector",
        ConversionPlan::Unsupported { .. } => "UNSUPPORTED",
    }
}

/// Parses a CQL type name, falling back to an opaque custom type.
///
/// A type `cdm-codec` cannot parse is not a reason to refuse the whole diff: the operator is
/// looking at this command precisely because something is unusual, and one unparseable column
/// should cost them that column's detail rather than the report.
fn parse_type(text: &str) -> CqlTypeInfo {
    CqlTypeInfo::parse(text).unwrap_or_else(|_| CqlTypeInfo::Custom(text.to_owned()))
}

fn describe(table: &TableSchema) -> TableReport {
    use cdm_cql::schema::table::ColumnKind;

    TableReport {
        table: table.quoted_name(),
        counter_table: table.is_counter_table(),
        columns: table
            .columns
            .iter()
            .map(|column| ColumnReport {
                name: column.name.clone(),
                cql_type: column.cql_type.clone(),
                kind: match column.kind {
                    ColumnKind::PartitionKey => "partition_key",
                    ColumnKind::Clustering => "clustering",
                    ColumnKind::Static => "static",
                    ColumnKind::Regular => "regular",
                }
                .to_owned(),
            })
            .collect(),
    }
}

/// The schema commands take only configuration; the harness takes a job's arguments.
fn job_args(config: &ConfigArgs) -> JobArgs {
    JobArgs {
        config: config.clone(),
        dry_run: false,
        summary_out: None,
        tui: false,
    }
}
