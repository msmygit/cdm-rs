//! The target write: `INSERT` for an ordinary table, `UPDATE … SET c = c + ?` for a counter one.
//!
//! # Two shapes, one type
//!
//! `SCH-005` makes the choice for the operator: a counter column anywhere on the target table means
//! the write path is an `UPDATE`, because `INSERT` on a counter table is rejected outright. The
//! bind order differs too — `MIG-011` puts TTL and writetime *last* for the `INSERT`, `MIG-030`
//! puts them *first* for the `UPDATE`, because in the `UPDATE` they precede the `SET` list in the
//! statement text and bind markers are positional.
//!
//! Rather than leave that as two hand-maintained loops, both shapes compile down to one ordered
//! [`BindSlot`] vector, and the binder walks it. `MIG-011` is then a property of a `Vec` that a
//! test can assert on directly, rather than of control flow.
//!
//! # A Java quirk deliberately not reproduced
//!
//! `TargetUpdateStatement.buildStatement` emits the separating comma only when at least one *bound*
//! `SET` entry has been written, so a constant column appearing before any bound column produces
//! `SET a='x' b=?` — CQL the server rejects. It is unreachable in practice only because constant
//! columns tend to sort late. cdm-rs emits the comma between every pair of `SET` entries.
//! Reproducing a syntax error has no legitimate use, and `--compat-java` does not restore it.

use std::fmt;

use cdm_core::{CdmError, ErrorKind, Side};

use super::mapping::{ColumnMapping, TargetSource};
use super::{join, select::is_key_source};

/// Whether the statement carries `USING TTL` and/or `USING TIMESTAMP` (`FEA-046`, `MIG-010`).
///
/// The mirror of `cdm-feature`'s `UsingClause`, which is where the two booleans come from; it lives
/// on the other side of the dependency edge (`ARCHITECTURE.md` §3), so the value crosses as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsingClause {
    /// Whether `USING TTL ?` is emitted.
    pub ttl: bool,
    /// Whether `USING TIMESTAMP ?` is emitted.
    pub timestamp: bool,
}

impl UsingClause {
    /// Whether the clause contributes anything at all.
    pub const fn is_empty(self) -> bool {
        !self.ttl && !self.timestamp
    }
}

impl fmt::Display for UsingClause {
    /// Renders exactly as Java's `TargetUpsertStatement.usingTTLTimestamp` does, leading space
    /// included, so a statement logged by either tool is byte-identical.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.ttl, self.timestamp) {
            (false, false) => Ok(()),
            (true, false) => f.write_str(" USING TTL ?"),
            (false, true) => f.write_str(" USING TIMESTAMP ?"),
            (true, true) => f.write_str(" USING TTL ? AND TIMESTAMP ?"),
        }
    }
}

/// What the write statement needs beyond the column mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatementOptions {
    /// Whether TTL and writetime are carried (`FEA-046`).
    pub using: UsingClause,
}

/// One bind marker of the target statement, in the order the markers appear (`MIG-011`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindSlot {
    /// The value of the target column at this index.
    Column(usize),
    /// The row's TTL, in seconds.
    Ttl,
    /// The row's writetime, in microseconds.
    Writetime,
    /// A primary-key component in the `WHERE` clause of an `UPDATE`.
    KeyColumn(usize),
}

/// The target write statement (`MIG-010`, `MIG-030`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetUpsert {
    cql: String,
    counter: bool,
    using: UsingClause,
    slots: Vec<BindSlot>,
}

impl TargetUpsert {
    /// Builds the statement the mapping implies.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] when a counter table's primary key is not fully derivable, or
    /// when a counter table's `UPDATE` would have no `SET` entry at all — a statement that writes
    /// nothing is never what the operator meant, and a counter write that silently does nothing is
    /// indistinguishable from one that worked.
    pub fn new(mapping: &ColumnMapping, options: StatementOptions) -> Result<Self, CdmError> {
        if mapping.target_is_counter() {
            Self::update(mapping, options)
        } else {
            Ok(Self::insert(mapping, options))
        }
    }

    /// The statement text.
    pub fn cql(&self) -> &str {
        &self.cql
    }

    /// Whether this is the counter `UPDATE` form (`SCH-005`).
    ///
    /// The engine reads this to refuse batching and retrying: a counter update is not idempotent,
    /// so a retry double-counts (`CON-012`, `MIG-032`). [`Binder`](super::Binder) enforces that
    /// structurally — a counter binding produces a [`CounterWrite`](super::CounterWrite), which
    /// does not implement [`Idempotent`](super::Idempotent) — so this accessor is for reporting,
    /// not for a runtime guard somebody can delete.
    pub const fn is_counter(&self) -> bool {
        self.counter
    }

    /// Whether TTL and writetime are carried.
    pub const fn using(&self) -> UsingClause {
        self.using
    }

    /// How many bind markers the statement has.
    pub fn bind_count(&self) -> usize {
        self.slots.len()
    }

    /// The bind markers, in statement order (`MIG-011`).
    pub(super) fn slots(&self) -> &[BindSlot] {
        &self.slots
    }

    /// Where the target column at `target_index` puts its value in a bound write (`MIG-022`).
    ///
    /// `None` for a constant column, which `MIG-010` inlines as a literal and which therefore
    /// occupies no bind position at all.
    ///
    /// Exposed because `MIG-022`'s partition grouping has to compare the partition-key components
    /// of two bound writes, and it cannot do that without knowing where they landed.
    #[must_use]
    pub fn bind_position(&self, target_index: usize) -> Option<usize> {
        self.slots.iter().position(|slot| {
            matches!(slot, BindSlot::Column(index) | BindSlot::KeyColumn(index)
                if *index == target_index)
        })
    }

    /// `INSERT INTO ks.tbl (bound…, const…) VALUES (?, …, <literals>)[ USING …]` (`MIG-010`).
    fn insert(mapping: &ColumnMapping, options: StatementOptions) -> Self {
        let mut bound_names = Vec::new();
        let mut constant_names = Vec::new();
        let mut constant_values = Vec::new();
        let mut slots = Vec::new();

        for (index, column) in mapping.target_columns().iter().enumerate() {
            if let Some(TargetSource::Constant(literal)) = mapping.source(index) {
                constant_names.push(column.quoted_name());
                constant_values.push(literal.clone());
            } else {
                bound_names.push(column.quoted_name());
                slots.push(BindSlot::Column(index));
            }
        }

        let mut values = vec!["?"; bound_names.len()].join(",");
        if !constant_values.is_empty() {
            if !values.is_empty() {
                values.push(',');
            }
            values.push_str(&join(&constant_values));
        }
        let mut columns = join(&bound_names);
        if !constant_names.is_empty() {
            if !columns.is_empty() {
                columns.push(',');
            }
            columns.push_str(&join(&constant_names));
        }

        if options.using.ttl {
            slots.push(BindSlot::Ttl);
        }
        if options.using.timestamp {
            slots.push(BindSlot::Writetime);
        }

        Self {
            cql: format!(
                "INSERT INTO {} ({columns}) VALUES ({values}){}",
                mapping.target_table().quoted_name(),
                options.using
            ),
            counter: false,
            using: options.using,
            slots,
        }
    }

    /// `UPDATE ks.tbl[ USING …] SET c = c + ?, … WHERE <pk>` (`MIG-030`).
    fn update(mapping: &ColumnMapping, options: StatementOptions) -> Result<Self, CdmError> {
        let mut slots = Vec::new();
        if options.using.ttl {
            slots.push(BindSlot::Ttl);
        }
        if options.using.timestamp {
            slots.push(BindSlot::Writetime);
        }

        let mut assignments = Vec::new();
        for (index, column) in mapping.target_columns().iter().enumerate() {
            if column.kind.is_key() {
                continue;
            }
            let name = column.quoted_name();
            if let Some(TargetSource::Constant(literal)) = mapping.source(index) {
                assignments.push(format!("{name}={literal}"));
            } else {
                if column.is_counter() {
                    assignments.push(format!("{name}={name}+?"));
                } else {
                    assignments.push(format!("{name}=?"));
                }
                slots.push(BindSlot::Column(index));
            }
        }

        if assignments.is_empty() {
            return Err(CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "the counter table {} has no non-key column to update, so the generated \
                     statement would write nothing (SCH-005).",
                    mapping.target_table().quoted_name()
                ),
            )
            .with_context(|c| {
                c.with_side(Side::Target)
                    .with_table(mapping.target_table().table_ref())
            }));
        }

        let (predicate, _) = where_clause(mapping)?;
        for column in mapping.target_table().primary_key() {
            let Some(index) = mapping
                .target_columns()
                .iter()
                .position(|c| c.name == column.name)
            else {
                continue;
            };
            if matches!(mapping.source(index), Some(TargetSource::Constant(_))) {
                continue;
            }
            slots.push(BindSlot::KeyColumn(index));
        }

        Ok(Self {
            cql: format!(
                "UPDATE {}{} SET {} WHERE {predicate}",
                mapping.target_table().quoted_name(),
                options.using,
                assignments.join(",")
            ),
            counter: true,
            using: options.using,
            slots,
        })
    }
}

/// The target `WHERE` clause over the primary key, and the key columns that carry a bind marker.
///
/// A constant primary-key component is spliced as a literal rather than bound, exactly as Java
/// splices `targetDefaultValueStrings`: the value is the same for every row, so binding it would
/// add a parameter to every statement for nothing (`FEA-012`).
pub(super) fn where_clause(mapping: &ColumnMapping) -> Result<(String, Vec<String>), CdmError> {
    let mut terms = Vec::new();
    let mut bound = Vec::new();
    for column in mapping.target_table().primary_key() {
        let source = mapping.source_of(&column.name);
        match source {
            Some(TargetSource::Constant(literal)) => {
                terms.push(format!("{}={literal}", column.quoted_name()));
            }
            Some(source) if is_key_source(source) => {
                terms.push(format!("{}=?", column.quoted_name()));
                bound.push(column.name.clone());
            }
            _ => {
                return Err(CdmError::new(
                    ErrorKind::SchemaMismatch,
                    format!(
                        "target primary-key column `{}` of {} has no source, so no WHERE clause \
                         can identify the row (SCH-006).",
                        column.name,
                        mapping.target_table().quoted_name()
                    ),
                )
                .with_context(|c| {
                    c.with_side(Side::Target)
                        .with_table(mapping.target_table().table_ref())
                        .with_column(column.name.clone())
                }));
            }
        }
    }
    Ok((terms.join(" AND "), bound))
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
    use super::*;
    use crate::schema::table::tests::column;
    use crate::schema::{ColumnKind, TableSchema};
    use crate::statement::mapping::tests::{origin, target};
    use crate::statement::MappingOptions;

    pub(crate) fn counter_target() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "hits".to_owned(),
            columns: vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("cc", "text", ColumnKind::Clustering, 0),
                column("n", "counter", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    pub(crate) fn counter_origin() -> TableSchema {
        let mut table = counter_target();
        table.table = "hits_src".to_owned();
        table
    }

    fn upsert(options: &MappingOptions, using: UsingClause) -> TargetUpsert {
        let mapping = ColumnMapping::resolve(&origin(), &target(), options).unwrap();
        TargetUpsert::new(&mapping, StatementOptions { using }).unwrap()
    }

    #[test]
    fn mig_010_the_insert_binds_mapped_columns_and_inlines_constants() {
        let mut target = target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::Regular, -1));
        let options = MappingOptions {
            rename: vec!["data:payload".to_owned()],
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin(), &target, &options).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();

        assert_eq!(
            statement.cql(),
            "INSERT INTO ks.dst (id,cc,payload,notes,tenant) VALUES (?,?,?,?,'acme')"
        );
        assert!(!statement.is_counter());
        assert_eq!(statement.bind_count(), 4);
    }

    #[test]
    fn mig_010_the_using_clause_is_appended_in_javas_exact_spelling() {
        let cases = [
            (UsingClause::default(), ""),
            (
                UsingClause {
                    ttl: true,
                    timestamp: false,
                },
                " USING TTL ?",
            ),
            (
                UsingClause {
                    ttl: false,
                    timestamp: true,
                },
                " USING TIMESTAMP ?",
            ),
            (
                UsingClause {
                    ttl: true,
                    timestamp: true,
                },
                " USING TTL ? AND TIMESTAMP ?",
            ),
        ];
        for (using, suffix) in cases {
            let statement = upsert(&MappingOptions::default(), using);
            assert!(statement.cql().ends_with(suffix), "{}", statement.cql());
            assert_eq!(using.to_string(), suffix);
            assert_eq!(statement.using(), using);
        }
        assert!(UsingClause::default().is_empty());
    }

    #[test]
    fn mig_011_bind_order_is_columns_then_ttl_then_writetime() {
        let statement = upsert(
            &MappingOptions::default(),
            UsingClause {
                ttl: true,
                timestamp: true,
            },
        );
        assert_eq!(
            statement.slots(),
            [
                BindSlot::Column(0),
                BindSlot::Column(1),
                BindSlot::Column(2),
                BindSlot::Column(3),
                BindSlot::Ttl,
                BindSlot::Writetime,
            ]
        );
    }

    #[test]
    fn sch_005_a_counter_target_switches_the_write_path_to_update() {
        let mapping = ColumnMapping::resolve(
            &counter_origin(),
            &counter_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let statement = TargetUpsert::new(
            &mapping,
            StatementOptions {
                using: UsingClause::default(),
            },
        )
        .unwrap();

        assert!(statement.is_counter());
        assert_eq!(
            statement.cql(),
            "UPDATE ks.hits SET n=n+? WHERE id=? AND cc=?"
        );
        assert_eq!(
            statement.slots(),
            [
                BindSlot::Column(2),
                BindSlot::KeyColumn(0),
                BindSlot::KeyColumn(1)
            ],
            "MIG-030: non-key columns, then the WHERE binds"
        );
    }

    #[test]
    fn sch_005_a_counter_update_binds_ttl_and_writetime_first() {
        let mapping = ColumnMapping::resolve(
            &counter_origin(),
            &counter_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let statement = TargetUpsert::new(
            &mapping,
            StatementOptions {
                using: UsingClause {
                    ttl: true,
                    timestamp: true,
                },
            },
        )
        .unwrap();
        assert_eq!(
            statement.cql(),
            "UPDATE ks.hits USING TTL ? AND TIMESTAMP ? SET n=n+? WHERE id=? AND cc=?"
        );
        assert_eq!(
            statement.slots().first(),
            Some(&BindSlot::Ttl),
            "the USING clause precedes the SET list, and bind markers are positional"
        );
        assert_eq!(statement.slots()[1], BindSlot::Writetime);
    }

    #[test]
    fn sch_005_a_counter_table_with_nothing_to_set_is_refused() {
        let mut table = counter_target();
        table.columns.retain(|c| c.name != "n");
        table
            .columns
            .push(column("n", "counter", ColumnKind::Clustering, 1));
        let mapping =
            ColumnMapping::resolve(&table.clone(), &table, &MappingOptions::default()).unwrap();
        let err = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::SchemaMismatch);
        assert!(err.message().contains("write nothing"), "{err}");
    }

    #[test]
    fn mig_022_a_columns_bind_position_is_where_its_value_lands() {
        let mut target = target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::Regular, -1));
        let options = MappingOptions {
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin(), &target, &options).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();

        assert_eq!(statement.bind_position(0), Some(0), "id binds first");
        assert_eq!(statement.bind_position(3), Some(3));
        assert_eq!(
            statement.bind_position(4),
            None,
            "a constant is inlined and occupies no bind position"
        );
        assert_eq!(statement.bind_position(99), None);

        // The counter form puts the key columns in the WHERE clause, after the SET list.
        let counter = ColumnMapping::resolve(
            &counter_origin(),
            &counter_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let update = TargetUpsert::new(&counter, StatementOptions::default()).unwrap();
        assert_eq!(update.bind_position(2), Some(0), "the counter binds first");
        assert_eq!(update.bind_position(0), Some(1), "then the WHERE clause");
    }

    #[test]
    fn mig_010_a_constant_in_the_counter_set_list_is_inlined_with_a_proper_comma() {
        let mut table = counter_target();
        table
            .columns
            .push(column("src", "text", ColumnKind::Regular, -1));
        let options = MappingOptions {
            constants: vec![("src".to_owned(), "'legacy'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&counter_origin(), &table, &options).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        assert_eq!(
            statement.cql(),
            "UPDATE ks.hits SET n=n+?,src='legacy' WHERE id=? AND cc=?",
            "Java would omit this comma when the constant sorts first; that is a syntax error"
        );
    }
}
