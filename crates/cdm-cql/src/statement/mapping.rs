//! Which origin column feeds which target column (`SCH-003`, `SCH-004`, `SCH-006`).
//!
//! # What the mapping decides
//!
//! Three questions, all answered once at startup:
//!
//! 1. **What does the origin `SELECT` project?** Every origin column except the skipped ones
//!    (`SCH-004`).
//! 2. **Where does each target column's value come from?** A mapped origin column, a constant
//!    literal, one half of an exploded map entry, an extracted JSON property, or nothing at all
//!    (`SCH-003`, `FEA-011`, `FEA-021`, `FEA-031`).
//! 3. **Is every target primary-key component derivable?** If not, the run must fail before it
//!    writes anything, naming the component (`SCH-006`).
//!
//! # Java parity, and one deliberate divergence
//!
//! Java builds the same correspondence in `CqlTable.calcCorrespondingIndex` and
//! `DataUtility.getThisToThatColumnNameMap`: explicit `origin:target` pairs first, then every
//! remaining identically-named column. Both sides of a pair go through `unFormatName`, and a pair
//! naming a column that is not on its table throws. All of that is reproduced.
//!
//! The divergence is `schema.origin.column.skip`. Java compares the configured name against
//! `ColumnMetadata.getName().asCql(true)` — the *quoted* form — so skipping a mixed-case column
//! requires the operator to write `"MyCol"` with the quotes, and skipping it by its plain name
//! silently does nothing. cdm-rs matches on the internal name and accepts the quoted spelling too,
//! so both work. A skip that silently does not skip is not a behaviour worth preserving: the
//! failure mode is a column migrating that the operator asked to leave behind.

use cdm_core::{CdmError, ErrorKind, Side};

use crate::schema::{identifier, ColumnMeta, TableSchema};

/// Where one target column's value comes from (`SCH-003`, `SCH-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSource {
    /// The origin column at this position in the origin projection.
    Origin(usize),
    /// A constant column, inlined into the statement as the CQL literal it carries (`FEA-010`).
    Constant(String),
    /// The key of an exploded map entry (`FEA-020`).
    ExplodeKey,
    /// The value of an exploded map entry (`FEA-020`).
    ExplodeValue,
    /// A property extracted from the JSON document in the origin column at this position
    /// (`FEA-030`).
    ExtractJson(usize),
    /// Nothing on the origin supplies this column, so it is left `UNSET` rather than written as
    /// `NULL` (`MIG-012`).
    Absent,
}

impl TargetSource {
    /// Whether the source consumes a bind marker in the target statement.
    ///
    /// A constant does not: `MIG-010` inlines it as a literal, so it takes a slot in the column
    /// list but none in the values list. Everything else, `Absent` included, does — an absent
    /// column still gets a marker, which binding sets to `UNSET`.
    pub const fn is_bound(&self) -> bool {
        !matches!(self, Self::Constant(_))
    }
}

/// The configuration a mapping is resolved from.
///
/// Every field is the projection of something a feature already computed, which is what keeps this
/// crate free of a `cdm-feature` dependency (see the [module docs](super)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingOptions {
    /// `schema.origin.column.rename`, each entry written `origin_name:target_name` (`SCH-003`).
    pub rename: Vec<String>,
    /// `schema.origin.column.skip` (`SCH-004`).
    pub skip: Vec<String>,
    /// Constant columns as `(target column, CQL literal)`, in configuration order (`FEA-010`).
    pub constants: Vec<(String, String)>,
    /// The explode map as `(origin map column, target key column, target value column)`
    /// (`FEA-020`).
    pub explode_map: Option<(String, String, String)>,
    /// Extract-JSON as `(origin document column, target column)` (`FEA-030`).
    pub extract_json: Option<(String, String)>,
}

/// The resolved origin-to-target correspondence (`SCH-003`, `SCH-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMapping {
    origin: Vec<ColumnMeta>,
    target: Vec<ColumnMeta>,
    sources: Vec<TargetSource>,
    origin_table: TableSchema,
    target_table: TableSchema,
}

impl ColumnMapping {
    /// Resolves the mapping, reporting the first problem that makes the run unsafe.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] for a malformed or dangling `rename` pair and for a skipped
    /// primary-key column; [`ErrorKind::SchemaMismatch`] for a target primary-key component that
    /// nothing supplies (`SCH-006`). All three are Tier-3 failures: they are only detectable once
    /// both schemas are known, and every one of them would otherwise surface as a server-side
    /// rejection on the first row of a multi-hour run.
    pub fn resolve(
        origin_table: &TableSchema,
        target_table: &TableSchema,
        options: &MappingOptions,
    ) -> Result<Self, CdmError> {
        let origin = Self::project_origin(origin_table, &options.skip)?;
        let renames = Self::parse_renames(&options.rename, &origin, target_table)?;

        let mut sources = Vec::with_capacity(target_table.columns.len());
        for column in &target_table.columns {
            sources.push(Self::source_for(column, &origin, &renames, options));
        }

        let mapping = Self {
            origin,
            target: target_table.columns.clone(),
            sources,
            origin_table: origin_table.clone(),
            target_table: target_table.clone(),
        };
        mapping.check_primary_key_is_derivable()?;
        Ok(mapping)
    }

    /// The origin columns the run reads, in projection order (`SCH-004`).
    pub fn origin_columns(&self) -> &[ColumnMeta] {
        &self.origin
    }

    /// The target columns, in schema order. Every one has a [`TargetSource`].
    pub fn target_columns(&self) -> &[ColumnMeta] {
        &self.target
    }

    /// Where the target column at `index` takes its value from.
    pub fn source(&self, index: usize) -> Option<&TargetSource> {
        self.sources.get(index)
    }

    /// Where the named target column takes its value from.
    pub fn source_of(&self, column: &str) -> Option<&TargetSource> {
        let index = self.target.iter().position(|c| c.name == column)?;
        self.sources.get(index)
    }

    /// The origin table, as introspected.
    pub const fn origin_table(&self) -> &TableSchema {
        &self.origin_table
    }

    /// The target table, as introspected.
    pub const fn target_table(&self) -> &TableSchema {
        &self.target_table
    }

    /// The position of the named column in the origin projection (`SCH-004`).
    pub fn origin_index_of(&self, column: &str) -> Option<usize> {
        self.origin.iter().position(|c| c.name == column)
    }

    /// Whether the target is a counter table, which switches the write path from `INSERT` to
    /// `UPDATE … SET c = c + ?` (`SCH-005`, `MIG-030`).
    pub fn target_is_counter(&self) -> bool {
        self.target_table.is_counter_table()
    }

    /// The origin columns, minus the skipped ones, in `system_schema` order (`SCH-004`).
    fn project_origin(table: &TableSchema, skip: &[String]) -> Result<Vec<ColumnMeta>, CdmError> {
        let skipped: Vec<String> = skip
            .iter()
            .map(|name| identifier::unformat(name.trim()))
            .filter(|name| !name.is_empty())
            .collect();

        for name in &skipped {
            match table.column(name) {
                Some(column) if column.kind.is_key() => {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "schema.origin.column.skip names `{name}`, which is a primary-key \
                             column of {}: a row cannot be identified without it (SCH-004).",
                            table.quoted_name()
                        ),
                    )
                    .with_context(|c| {
                        c.with_side(Side::Origin)
                            .with_table(table.table_ref())
                            .with_column(name.clone())
                    }));
                }
                Some(_) => {}
                None => {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "schema.origin.column.skip names `{name}`, which is not a column of \
                             {} (SCH-004).",
                            table.quoted_name()
                        ),
                    )
                    .with_context(|c| {
                        c.with_side(Side::Origin)
                            .with_table(table.table_ref())
                            .with_column(name.clone())
                    }));
                }
            }
        }

        Ok(table
            .columns
            .iter()
            .filter(|column| !skipped.contains(&column.name))
            .cloned()
            .collect())
    }

    /// The explicit `origin:target` pairs, validated against both sides (`SCH-003`).
    fn parse_renames(
        rename: &[String],
        origin: &[ColumnMeta],
        target: &TableSchema,
    ) -> Result<Vec<(String, String)>, CdmError> {
        let mut pairs = Vec::with_capacity(rename.len());
        for entry in rename {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let mut halves = entry.splitn(2, ':');
            let left = halves.next().unwrap_or_default().trim();
            let right = halves.next().unwrap_or_default().trim();
            if left.is_empty() || right.is_empty() {
                return Err(CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "schema.origin.column.rename entry `{entry}` is mis-configured: it must \
                         be written `origin_name:target_name` with both sides present (SCH-003).",
                    ),
                ));
            }
            let origin_name = identifier::unformat(left);
            let target_name = identifier::unformat(right);

            if !origin.iter().any(|c| c.name == origin_name) {
                return Err(Self::dangling_rename(
                    entry,
                    &origin_name,
                    Side::Origin,
                    target,
                ));
            }
            if target.column(&target_name).is_none() {
                return Err(Self::dangling_rename(
                    entry,
                    &target_name,
                    Side::Target,
                    target,
                ));
            }
            pairs.push((origin_name, target_name));
        }
        Ok(pairs)
    }

    fn dangling_rename(entry: &str, column: &str, side: Side, target: &TableSchema) -> CdmError {
        CdmError::new(
            ErrorKind::Config,
            format!(
                "schema.origin.column.rename entry `{entry}` names `{column}`, which is not a \
                 column of the {side} table (SCH-003)."
            ),
        )
        .with_context(|c| {
            c.with_side(side)
                .with_table(target.table_ref())
                .with_column(column.to_owned())
        })
    }

    /// Resolution order matches Java's bind loop: explode-map key, explode-map value, extracted
    /// JSON, constant, then the mapped origin column.
    ///
    /// The order matters where two features name the same target column. A constant losing to an
    /// explode-map column is what `FEA-022` needs; a constant winning over a plain origin column is
    /// what `FEA-014` means by "origin constants are replaceable by different target constants".
    fn source_for(
        column: &ColumnMeta,
        origin: &[ColumnMeta],
        renames: &[(String, String)],
        options: &MappingOptions,
    ) -> TargetSource {
        if let Some((_, key, value)) = &options.explode_map {
            if identifier::unformat(key) == column.name {
                return TargetSource::ExplodeKey;
            }
            if identifier::unformat(value) == column.name {
                return TargetSource::ExplodeValue;
            }
        }
        if let Some((document, extracted)) = &options.extract_json {
            if identifier::unformat(extracted) == column.name {
                let document = identifier::unformat(document);
                if let Some(index) = origin.iter().position(|c| c.name == document) {
                    return TargetSource::ExtractJson(index);
                }
                return TargetSource::Absent;
            }
        }
        if let Some((_, literal)) = options
            .constants
            .iter()
            .find(|(name, _)| identifier::unformat(name) == column.name)
        {
            return TargetSource::Constant(literal.clone());
        }
        if let Some((origin_name, _)) = renames.iter().find(|(_, to)| *to == column.name) {
            if let Some(index) = origin.iter().position(|c| c.name == *origin_name) {
                return TargetSource::Origin(index);
            }
        }
        // Identity fallback, exactly as Java's `DataUtility.getThisToThatColumnNameMap` does it:
        // after the explicit pairs, every remaining identically-named column maps to itself. Note
        // that renaming `a:b` where the target also has an `a` therefore writes the origin's `a`
        // into *both* target columns — Java does that too, and `SCH-003` is a parity requirement.
        origin
            .iter()
            .position(|c| c.name == column.name)
            .map_or(TargetSource::Absent, TargetSource::Origin)
    }

    /// Every target primary-key component must have a source (`SCH-006`).
    fn check_primary_key_is_derivable(&self) -> Result<(), CdmError> {
        for column in self.target_table.primary_key() {
            let source = self.source_of(&column.name);
            if !matches!(source, Some(TargetSource::Absent) | None) {
                continue;
            }
            return Err(CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "target primary-key column `{}` of {} has no source: no origin column maps to \
                     it, no constant column supplies it and the explode map does not produce it. \
                     Map an origin column with schema.origin.column.rename, or supply it with \
                     feature.constant_columns (SCH-006).",
                    column.name,
                    self.target_table.quoted_name()
                ),
            )
            .with_context(|c| {
                c.with_side(Side::Target)
                    .with_table(self.target_table.table_ref())
                    .with_column(column.name.clone())
            }));
        }
        Ok(())
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
pub(crate) mod tests {
    use super::*;
    use crate::schema::table::tests::column;
    use crate::schema::ColumnKind;

    pub(crate) fn origin() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "src".to_owned(),
            columns: vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("cc", "text", ColumnKind::Clustering, 0),
                column("data", "text", ColumnKind::Regular, -1),
                column("notes", "text", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    pub(crate) fn target() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "dst".to_owned(),
            columns: vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("cc", "text", ColumnKind::Clustering, 0),
                column("payload", "text", ColumnKind::Regular, -1),
                column("notes", "text", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    fn resolve(options: &MappingOptions) -> Result<ColumnMapping, CdmError> {
        ColumnMapping::resolve(&origin(), &target(), options)
    }

    #[test]
    fn sch_003_identically_named_columns_map_automatically() {
        let mapping = resolve(&MappingOptions::default()).unwrap();
        assert_eq!(mapping.source_of("id"), Some(&TargetSource::Origin(0)));
        assert_eq!(mapping.source_of("cc"), Some(&TargetSource::Origin(1)));
        assert_eq!(mapping.source_of("notes"), Some(&TargetSource::Origin(3)));
        assert_eq!(
            mapping.source_of("payload"),
            Some(&TargetSource::Absent),
            "no origin column is called payload, so it is left unset rather than nulled"
        );
    }

    #[test]
    fn sch_003_an_explicit_rename_wins_and_the_renamed_column_does_not_also_map_to_itself() {
        let options = MappingOptions {
            rename: vec!["data:payload".to_owned()],
            ..MappingOptions::default()
        };
        let mapping = resolve(&options).unwrap();
        assert_eq!(mapping.source_of("payload"), Some(&TargetSource::Origin(2)));
        assert_eq!(mapping.source_of("notes"), Some(&TargetSource::Origin(3)));
    }

    #[test]
    fn sch_003_a_rename_does_not_remove_the_identity_mapping_of_the_same_name() {
        let options = MappingOptions {
            rename: vec!["notes:payload".to_owned()],
            ..MappingOptions::default()
        };
        let mapping = resolve(&options).unwrap();
        assert_eq!(mapping.source_of("payload"), Some(&TargetSource::Origin(3)));
        assert_eq!(
            mapping.source_of("notes"),
            Some(&TargetSource::Origin(3)),
            "Java's name map adds the identity entry after the explicit pairs, so the origin's \
             `notes` lands in both target columns; SCH-003 is a parity requirement"
        );
    }

    #[test]
    fn sch_003_a_malformed_rename_pair_names_the_entry() {
        for entry in ["data", "data:", ":payload"] {
            let err = resolve(&MappingOptions {
                rename: vec![entry.to_owned()],
                ..MappingOptions::default()
            })
            .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Config);
            assert!(err.message().contains(entry), "{err}");
        }
    }

    #[test]
    fn sch_003_a_rename_naming_a_missing_column_reports_the_column_and_the_side() {
        let err = resolve(&MappingOptions {
            rename: vec!["nope:payload".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap_err();
        assert_eq!(err.context().side, Some(Side::Origin));
        assert!(err.message().contains("nope"), "{err}");

        let err = resolve(&MappingOptions {
            rename: vec!["data:nope".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap_err();
        assert_eq!(err.context().side, Some(Side::Target));
        assert!(err.message().contains("nope"), "{err}");
    }

    #[test]
    fn sch_004_a_skipped_column_leaves_the_origin_projection() {
        let mapping = resolve(&MappingOptions {
            skip: vec!["notes".to_owned()],
            rename: vec!["data:payload".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap();
        let names: Vec<&str> = mapping
            .origin_columns()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["id", "cc", "data"]);
        assert_eq!(mapping.source_of("notes"), Some(&TargetSource::Absent));
        assert_eq!(mapping.origin_index_of("notes"), None);
    }

    #[test]
    fn sch_004_a_skip_written_in_its_quoted_form_is_understood() {
        let mapping = resolve(&MappingOptions {
            skip: vec!["\"notes\"".to_owned()],
            rename: vec!["data:payload".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap();
        assert_eq!(mapping.origin_index_of("notes"), None);
    }

    #[test]
    fn sch_004_skipping_a_primary_key_column_is_rejected() {
        let err = resolve(&MappingOptions {
            skip: vec!["cc".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("primary-key"), "{err}");
        assert_eq!(err.context().column.as_deref(), Some("cc"));
    }

    #[test]
    fn sch_004_skipping_a_column_that_does_not_exist_is_rejected() {
        let err = resolve(&MappingOptions {
            skip: vec!["nope".to_owned()],
            ..MappingOptions::default()
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("nope"), "{err}");
    }

    #[test]
    fn sch_006_a_constant_column_can_supply_a_target_primary_key_component() {
        let mut target = target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::PartitionKey, 1));
        let err =
            ColumnMapping::resolve(&origin(), &target, &MappingOptions::default()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::SchemaMismatch);
        assert!(err.message().contains("tenant"), "{err}");
        assert_eq!(err.context().side, Some(Side::Target));

        let options = MappingOptions {
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin(), &target, &options).unwrap();
        assert_eq!(
            mapping.source_of("tenant"),
            Some(&TargetSource::Constant("'acme'".to_owned()))
        );
        assert!(!mapping.source_of("tenant").unwrap().is_bound());
        assert!(mapping.source_of("id").unwrap().is_bound());
    }

    #[test]
    fn sch_006_an_explode_map_key_can_supply_a_target_primary_key_component() {
        let mut origin = origin();
        origin
            .columns
            .push(column("m", "map<text, int>", ColumnKind::Regular, -1));
        let mut target = target();
        target
            .columns
            .push(column("k", "text", ColumnKind::Clustering, 1));
        target
            .columns
            .push(column("v", "int", ColumnKind::Regular, -1));

        let options = MappingOptions {
            explode_map: Some(("m".to_owned(), "k".to_owned(), "v".to_owned())),
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin, &target, &options).unwrap();
        assert_eq!(mapping.source_of("k"), Some(&TargetSource::ExplodeKey));
        assert_eq!(mapping.source_of("v"), Some(&TargetSource::ExplodeValue));
    }

    #[test]
    fn sch_003_extract_json_takes_its_value_from_the_document_column() {
        let mut target = target();
        target
            .columns
            .push(column("extracted", "text", ColumnKind::Regular, -1));
        let options = MappingOptions {
            extract_json: Some(("data".to_owned(), "extracted".to_owned())),
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin(), &target, &options).unwrap();
        assert_eq!(
            mapping.source_of("extracted"),
            Some(&TargetSource::ExtractJson(2))
        );
    }

    #[test]
    fn sch_005_a_counter_target_is_detected_from_the_schema() {
        let mapping = resolve(&MappingOptions::default()).unwrap();
        assert!(!mapping.target_is_counter());
        assert_eq!(mapping.target_columns().len(), 4);
        assert_eq!(mapping.origin_table().table, "src");
        assert_eq!(mapping.target_table().table, "dst");
        assert!(mapping.source(99).is_none());
    }
}
