//! Explode map — one origin map column becomes one target row per entry (`FEA-020`..`FEA-023`).
//!
//! # What the feature is for
//!
//! It converts a denormalised map into rows: `{'a': 1, 'b': 2}` in one origin row becomes two
//! target rows whose key and value columns hold the entry. Because the exploded key is usually a
//! clustering column (`FEA-022`), the feature is also the reason a single origin row can produce
//! several target primary keys, and therefore the reason `FeaturePlugin::transform` emits into a
//! sink rather than returning one record.
//!
//! # Java parity
//!
//! `com.datastax.cdm.feature.ExplodeMap` collects the exploded entries into a `Set`, which loses
//! their order; cdm-rs preserves wire order, which for a Cassandra map is key order. That is not a
//! behavioural difference in what is written — the same entries reach the same target rows — but it
//! does make a run reproducible, which matters when comparing two runs' logs.

use cdm_codec::{ConversionPlan, CqlTypeInfo, Planner};
use cdm_core::{
    CdmError, Diagnostic, EffectiveConfig, ErrorKind, FeaturePlugin, Plugin, RawCell, Record,
    RecordSink, Row, SchemaPair,
};

use crate::properties::{
    self, EXPLODE_MAP_ORIGIN_COLUMN, EXPLODE_MAP_TARGET_KEY_COLUMN, EXPLODE_MAP_TARGET_VALUE_COLUMN,
};
use crate::schema::{FeatureSchema, TableFacts};
use crate::wire::map_entries;
use crate::{diagnostic, PROVIDER};

/// One exploded map entry, converted to the target column types (`FEA-021`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplodedEntry {
    /// The map key, as the target key column's type.
    pub key: RawCell,
    /// The map value, as the target value column's type.
    pub value: RawCell,
}

/// The explode-map feature's configuration (`FEA-020`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExplodeMap {
    origin: String,
    key: String,
    value: String,
}

impl ExplodeMap {
    /// Reads the feature's configuration.
    ///
    /// All three names or none: a partial configuration is a validation finding rather than a load
    /// failure, so that the operator sees it alongside everything else that is wrong (`CFG-031`).
    pub fn load(config: &EffectiveConfig) -> Self {
        Self {
            origin: properties::trimmed(config, EXPLODE_MAP_ORIGIN_COLUMN).unwrap_or_default(),
            key: properties::trimmed(config, EXPLODE_MAP_TARGET_KEY_COLUMN).unwrap_or_default(),
            value: properties::trimmed(config, EXPLODE_MAP_TARGET_VALUE_COLUMN).unwrap_or_default(),
        }
    }

    /// Whether all three column names are configured.
    pub fn is_enabled(&self) -> bool {
        !self.origin.is_empty() && !self.key.is_empty() && !self.value.is_empty()
    }

    /// The origin map column.
    pub fn origin_column(&self) -> &str {
        &self.origin
    }

    /// The target column the map key is written to.
    pub fn key_column(&self) -> &str {
        &self.key
    }

    /// The target column the map value is written to.
    pub fn value_column(&self) -> &str {
        &self.value
    }

    /// Validates the configuration against both schemas (`FEA-020`).
    pub fn validate(&self, schema: &FeatureSchema) -> Vec<Diagnostic> {
        let mut findings = self.validate_completeness();
        if !findings.is_empty() || !self.is_enabled() {
            return findings;
        }

        match schema.origin.column(&self.origin) {
            None => findings.push(
                diagnostic::schema_error(format!(
                    "explode-map origin column `{}` is not on the origin table {}",
                    self.origin,
                    schema.origin.table()
                ))
                .with_rule("FEA-020"),
            ),
            Some(column) if !matches!(column.cql_type(), CqlTypeInfo::Map { .. }) => findings.push(
                diagnostic::schema_error(format!(
                    "explode-map origin column `{}` is {}, not a map",
                    self.origin,
                    column.cql_type()
                ))
                .with_rule("FEA-020"),
            ),
            Some(_) => {}
        }

        for (role, name) in [("key", &self.key), ("value", &self.value)] {
            if schema.target.column(name).is_none() {
                findings.push(
                    diagnostic::schema_error(format!(
                        "explode-map target {role} column `{name}` is not on the target table {}",
                        schema.target.table()
                    ))
                    .with_rule("FEA-020"),
                );
            }
        }
        findings
    }

    /// The all-or-nothing rule of `CFG-031`.
    fn validate_completeness(&self) -> Vec<Diagnostic> {
        let configured = [&self.origin, &self.key, &self.value]
            .into_iter()
            .filter(|name| !name.is_empty())
            .count();
        if configured == 0 || configured == 3 {
            return Vec::new();
        }
        vec![diagnostic::config_error(
            "explode map requires the origin column, the target key column and the target value \
             column, or none of them",
        )
        .with_rule("FEA-020")]
    }

    /// Resolves the origin column's position and the two element conversions (`FEA-021`).
    ///
    /// The conversions are planned once here rather than per row, which is the rule the whole
    /// pipeline follows (`CDC-010`): a map with a thousand entries then costs a thousand
    /// applications of an already-resolved plan, not a thousand plan lookups.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`] when a column is missing or the origin column is not a
    /// map — the same conditions [`ExplodeMap::validate`] reports as diagnostics, so a caller that
    /// validated first cannot reach this.
    pub fn resolve(
        &self,
        schema: &FeatureSchema,
        planner: &Planner,
    ) -> Result<ExplodePlan, CdmError> {
        let origin = schema
            .origin
            .column(&self.origin)
            .ok_or_else(|| missing(&self.origin, &schema.origin.table().to_string()))?;
        let CqlTypeInfo::Map { key, value, .. } = origin.cql_type() else {
            return Err(CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "explode-map origin column `{}` is {}, not a map",
                    self.origin,
                    origin.cql_type()
                ),
            )
            .with_context(|c| c.with_column(self.origin.clone())));
        };
        let origin_index = schema
            .origin
            .index_of(&self.origin)
            .ok_or_else(|| missing(&self.origin, &schema.origin.table().to_string()))?;

        let key_column = target_column(&schema.target, &self.key)?;
        let value_column = target_column(&schema.target, &self.value)?;

        Ok(ExplodePlan {
            origin_index,
            key_plan: planner.plan_types(key, key_column.cql_type()),
            value_plan: planner.plan_types(value, value_column.cql_type()),
            key_column: self.key.clone(),
            value_column: self.value.clone(),
            key_is_key: key_column.is_key(),
            value_is_key: value_column.is_key(),
        })
    }
}

/// The error a missing explode-map column produces.
fn missing(column: &str, table: &str) -> CdmError {
    CdmError::new(
        ErrorKind::SchemaMismatch,
        format!("explode-map column `{column}` is not on table {table}"),
    )
    .with_context(|c| c.with_column(column.to_owned()))
}

fn target_column<'a>(
    target: &'a TableFacts,
    name: &str,
) -> Result<&'a crate::schema::ColumnFacts, CdmError> {
    target.column(name).ok_or_else(|| {
        CdmError::new(
            ErrorKind::SchemaMismatch,
            format!(
                "explode-map column `{name}` is not on table {}",
                target.table()
            ),
        )
        .with_context(|c| c.with_column(name.to_owned()))
    })
}

/// The resolved feature: positions and conversions, ready for the hot path (`FEA-021`).
#[derive(Debug)]
pub struct ExplodePlan {
    origin_index: usize,
    key_plan: ConversionPlan,
    value_plan: ConversionPlan,
    key_column: String,
    value_column: String,
    key_is_key: bool,
    value_is_key: bool,
}

impl ExplodePlan {
    /// The position of the map column in the origin projection.
    pub const fn origin_index(&self) -> usize {
        self.origin_index
    }

    /// The target column the key is written to.
    pub fn key_column(&self) -> &str {
        &self.key_column
    }

    /// The target column the value is written to.
    pub fn value_column(&self) -> &str {
        &self.value_column
    }

    /// Whether the exploded key is part of the target primary key (`FEA-022`).
    ///
    /// When it is — the usual case — each exploded entry yields a *distinct* target row, and the
    /// engine must derive one primary key per entry rather than reusing the origin row's.
    pub const fn key_is_primary_key(&self) -> bool {
        self.key_is_key
    }

    /// Whether the exploded value is part of the target primary key (`FEA-022`).
    pub const fn value_is_primary_key(&self) -> bool {
        self.value_is_key
    }

    /// Explodes one serialised map cell (`FEA-020`, `FEA-021`, `FEA-023`).
    ///
    /// A `null` or empty map yields no entries, which the caller counts as `SKIPPED`: there is no
    /// target row to write, and writing one with null key columns would be rejected by the cluster
    /// anyway.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] if the cell is not a well-formed map or an entry does
    /// not convert. Record-level: the engine counts `ERROR` for the row and carries on.
    pub fn explode(&self, cell: &RawCell) -> Result<Vec<ExplodedEntry>, CdmError> {
        let Some(bytes) = cell.bytes() else {
            return Ok(Vec::new());
        };
        let entries = map_entries(bytes)?;
        let mut exploded = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            exploded.push(ExplodedEntry {
                key: self.key_plan.apply(&key)?,
                value: self.value_plan.apply(&value)?,
            });
        }
        Ok(exploded)
    }

    /// Explodes the map column of an origin row.
    ///
    /// # Errors
    ///
    /// As [`ExplodePlan::explode`], plus [`ErrorKind::Internal`] if the row is shorter than the
    /// projection the plan was built from.
    pub fn explode_record(&self, record: &Record) -> Result<Vec<ExplodedEntry>, CdmError> {
        self.explode(record.origin_cell(self.origin_index)?)
    }
}

impl Plugin for ExplodePlan {
    fn name(&self) -> &'static str {
        "explode-map"
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl FeaturePlugin for ExplodePlan {
    fn is_enabled(&self, _config: &EffectiveConfig) -> bool {
        true
    }

    /// The plan exists only because [`ExplodeMap::validate`] already passed, so there is nothing
    /// left to say about the schema here.
    fn validate(&self, _config: &EffectiveConfig, _schema: &SchemaPair) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// Emits one record per map entry, with the converted key and value appended to the origin row
    /// in that order (`FEA-020`).
    ///
    /// Appending rather than substituting keeps every other column at the position the projection
    /// gave it, so the statement builder can address the exploded pair as the last two cells and
    /// nothing else has to move. An empty or null map emits nothing, which counts as `SKIPPED`
    /// (`FEA-023`).
    fn transform(&self, record: Record, out: &mut dyn RecordSink) -> Result<(), CdmError> {
        for entry in self.explode_record(&record)? {
            let mut cells = record.origin().cells().to_vec();
            cells.push(entry.key);
            cells.push(entry.value);
            out.emit(record.clone().with_origin(Row::new(cells)))?;
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
mod tests {
    use super::*;
    use crate::schema::table_view;
    use cdm_codec::{CodecRegistry, Codecset, PlannerOptions};
    use cdm_core::{PrimaryKey, TableRef};

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn enabled() -> EffectiveConfig {
        config(&[
            ("feature.explode_map.origin_column", "m"),
            ("feature.explode_map.target_key_column", "k"),
            ("feature.explode_map.target_value_column", "v"),
        ])
    }

    fn schema(key_type: &str, value_type: &str) -> FeatureSchema {
        let origin = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("m", "map<text, int>")],
            ),
            &["id"],
        )
        .unwrap();
        let target = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[("id", "int"), ("k", key_type), ("v", value_type)],
            ),
            &["id", "k"],
        )
        .unwrap();
        FeatureSchema::new(origin, target)
    }

    fn planner() -> Planner {
        Planner::new(
            CodecRegistry::with_builtins(&[Codecset::IntString], None).unwrap(),
            PlannerOptions::default(),
        )
    }

    fn map_cell(entries: &[(&str, i32)]) -> RawCell {
        let mut out = i32::try_from(entries.len()).unwrap().to_be_bytes().to_vec();
        for (key, value) in entries {
            out.extend_from_slice(&i32::try_from(key.len()).unwrap().to_be_bytes());
            out.extend_from_slice(key.as_bytes());
            out.extend_from_slice(&4_i32.to_be_bytes());
            out.extend_from_slice(&value.to_be_bytes());
        }
        RawCell::new(out)
    }

    #[test]
    fn fea_020_each_map_entry_becomes_one_target_row() {
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "int"), &planner())
            .unwrap();
        let entries = plan.explode(&map_cell(&[("a", 1), ("b", 2)])).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, RawCell::new(b"a".to_vec()));
        assert_eq!(entries[0].value, RawCell::new(1_i32.to_be_bytes().to_vec()));
        assert_eq!(entries[1].key, RawCell::new(b"b".to_vec()));
        assert_eq!(plan.origin_index(), 1);
        assert_eq!(plan.key_column(), "k");
        assert_eq!(plan.value_column(), "v");
    }

    #[test]
    fn fea_020_the_origin_column_must_exist_and_be_a_map() {
        let feature = ExplodeMap::load(&enabled());
        assert!(feature.is_enabled());
        assert!(feature.validate(&schema("text", "int")).is_empty());

        let not_a_map = TableFacts::from_view(
            &table_view(TableRef::new("ks", "src"), &[("id", "int"), ("m", "text")]),
            &["id"],
        )
        .unwrap();
        let schema = FeatureSchema::new(not_a_map, schema("text", "int").target);
        let findings = feature.validate(&schema);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("not a map"));
        assert_eq!(
            feature.resolve(&schema, &planner()).unwrap_err().kind(),
            ErrorKind::SchemaMismatch
        );
    }

    #[test]
    fn fea_020_a_partial_configuration_is_reported_and_no_configuration_is_off() {
        let partial = ExplodeMap::load(&config(&[("feature.explode_map.origin_column", "m")]));
        assert!(!partial.is_enabled());
        let findings = partial.validate(&schema("text", "int"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule.as_deref(), Some("FEA-020"));

        let off = ExplodeMap::load(&EffectiveConfig::new());
        assert!(!off.is_enabled());
        assert!(off.validate(&schema("text", "int")).is_empty());
        assert_eq!(off.origin_column(), "");
    }

    #[test]
    fn fea_020_a_missing_target_column_is_reported_for_both_roles() {
        let feature = ExplodeMap::load(&enabled());
        let target = TableFacts::from_view(
            &table_view(TableRef::new("ks", "dst"), &[("id", "int")]),
            &["id"],
        )
        .unwrap();
        let schema = FeatureSchema::new(schema("text", "int").origin, target);
        let findings = feature.validate(&schema);
        assert_eq!(findings.len(), 2);
        assert!(feature.resolve(&schema, &planner()).is_err());
    }

    #[test]
    fn fea_021_elements_are_converted_to_the_target_column_types() {
        // The target's key column is `text` and the value column is `text` too, so the map's
        // `int` values go through the INT_STRING codec on the way (CDC-010).
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "text"), &planner())
            .unwrap();
        let entries = plan.explode(&map_cell(&[("a", 10)])).unwrap();
        assert_eq!(entries[0].value, RawCell::new(b"10".to_vec()));
    }

    #[test]
    fn fea_022_the_exploded_key_and_value_may_be_part_of_the_target_primary_key() {
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "int"), &planner())
            .unwrap();
        assert!(plan.key_is_primary_key());
        assert!(!plan.value_is_primary_key());
    }

    #[test]
    fn fea_023_a_null_or_empty_map_produces_no_rows() {
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "int"), &planner())
            .unwrap();
        assert!(plan.explode(&RawCell::NULL).unwrap().is_empty());
        assert!(plan.explode(&map_cell(&[])).unwrap().is_empty());

        let record = Record::new(
            PrimaryKey::new(vec![RawCell::new(vec![0, 0, 0, 1])]),
            Row::new(vec![RawCell::new(vec![0, 0, 0, 1]), RawCell::NULL]),
        );
        let mut sink: Vec<Record> = Vec::new();
        plan.transform(record, &mut sink).unwrap();
        assert!(sink.is_empty(), "an empty map counts as SKIPPED, not ERROR");
    }

    #[test]
    fn fea_020_transform_appends_the_converted_key_and_value_to_each_record() {
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "int"), &planner())
            .unwrap();
        let record = Record::new(
            PrimaryKey::new(vec![RawCell::new(vec![0, 0, 0, 1])]),
            Row::new(vec![
                RawCell::new(vec![0, 0, 0, 1]),
                map_cell(&[("a", 1), ("b", 2)]),
            ]),
        );
        let mut sink: Vec<Record> = Vec::new();
        plan.transform(record, &mut sink).unwrap();
        assert_eq!(sink.len(), 2);
        assert_eq!(sink[0].origin().len(), 4);
        assert_eq!(sink[0].origin().get(2), Some(&RawCell::new(b"a".to_vec())));
        assert_eq!(
            sink[1].origin().get(3),
            Some(&RawCell::new(2_i32.to_be_bytes().to_vec()))
        );
        assert_eq!(Plugin::name(&plan), "explode-map");
        assert_eq!(Plugin::provider(&plan), PROVIDER);
        assert!(FeaturePlugin::is_enabled(&plan, &EffectiveConfig::new()));
        assert!(FeaturePlugin::validate(
            &plan,
            &EffectiveConfig::new(),
            &SchemaPair::new(
                table_view(TableRef::new("ks", "src"), &[]),
                table_view(TableRef::new("ks", "dst"), &[]),
            )
        )
        .is_empty());
    }

    #[test]
    fn fea_020_a_malformed_map_is_a_record_level_error() {
        let plan = ExplodeMap::load(&enabled())
            .resolve(&schema("text", "int"), &planner())
            .unwrap();
        let error = plan.explode(&RawCell::new(vec![0, 0])).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);

        let record = Record::new(PrimaryKey::default(), Row::new(vec![RawCell::NULL]));
        assert_eq!(
            plan.explode_record(&record).unwrap_err().kind(),
            ErrorKind::Internal,
            "a projection shorter than the plan is a bug, reported rather than panicked"
        );
    }
}
