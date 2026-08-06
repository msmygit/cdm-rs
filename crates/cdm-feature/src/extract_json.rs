//! Extract JSON — one property of a JSON document becomes a target column (`FEA-030`..`FEA-035`).
//!
//! # What the feature is for
//!
//! Origin schemas that stored a document in a `text` column are common, and the migration is often
//! the moment to promote one of its properties to a real column. The feature reads the named
//! property out of each row's document and writes it to the mapped target column, optionally
//! leaving an already-populated target alone (`FEA-032`) and optionally narrowing the whole run to
//! that one column (`FEA-033`).
//!
//! # Java parity, and two places cdm-rs diverges
//!
//! `com.datastax.cdm.feature.ExtractJson` splits `propertyMapping` on `:` into a JSON field name
//! and a target column, defaulting the column to the field name — reproduced exactly. Two
//! behaviours are deliberately different, both documented in `docs/MIGRATION_FROM_JAVA.md`:
//!
//! * **Malformed JSON** (`FEA-034`) is a record-level error here. In Java the Jackson exception
//!   propagates out of the bind and fails the entire token range, so one bad document costs the
//!   whole partition; counting `ERROR` for the row and logging its primary key loses nothing and
//!   keeps the run going.
//! * **`exclusive` column matching** (`FEA-033`) is exact here. Java filters the column list with
//!   `name.endsWith(extractColumn)`, so configuring `json` also retains a column called `oldjson` —
//!   a silent widening of a setting whose entire purpose is to narrow.
//!
//! The JSON-Pointer support of `FEA-035` is new in cdm-rs and strictly additive: a mapping that does
//! not begin with `/` is a top-level field name, exactly as in Java.

use cdm_codec::CqlTypeInfo;
use cdm_core::{
    CdmError, Diagnostic, EffectiveConfig, ErrorKind, FeaturePlugin, Plugin, RawCell, Record,
    SchemaPair,
};

use crate::literal::encode_json;
use crate::properties::{
    self, EXTRACT_JSON_EXCLUSIVE, EXTRACT_JSON_ORIGIN_COLUMN, EXTRACT_JSON_OVERWRITE,
    EXTRACT_JSON_PROPERTY_MAPPING,
};
use crate::schema::{FeatureSchema, TableFacts};
use crate::{diagnostic, PROVIDER};

/// How a mapping addresses a value inside the document (`FEA-030`, `FEA-035`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPath {
    /// A top-level property name, which is all Java supports.
    Field(String),
    /// An RFC 6901 JSON Pointer, e.g. `/address/city` or `/tags/0` (`FEA-035`).
    Pointer(String),
}

impl JsonPath {
    /// Interprets a mapping's left-hand side. A leading `/` means a pointer; anything else is a
    /// field name, so a document with a property literally called `a/b` still resolves.
    pub fn parse(text: &str) -> Self {
        if text.starts_with('/') {
            Self::Pointer(text.to_owned())
        } else {
            Self::Field(text.to_owned())
        }
    }

    /// Resolves the path against a document, returning `None` when it addresses nothing.
    pub fn resolve<'a>(&self, document: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
        match self {
            Self::Field(name) => document.get(name),
            Self::Pointer(pointer) => document.pointer(pointer),
        }
    }

    /// The path as configured.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Field(text) | Self::Pointer(text) => text,
        }
    }
}

/// The extract-JSON feature's configuration (`FEA-030`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractJson {
    origin_column: String,
    path: JsonPath,
    target_column: String,
    overwrite: bool,
    exclusive: bool,
}

impl ExtractJson {
    /// Reads the feature's configuration, splitting `property_mapping` into path and column.
    ///
    /// A bare `name` maps the top-level property `name` to the target column `name`, which is the
    /// shorthand Java supports and most configurations use.
    pub fn load(config: &EffectiveConfig) -> Self {
        let mapping =
            properties::trimmed(config, EXTRACT_JSON_PROPERTY_MAPPING).unwrap_or_default();
        let (path, target_column) = match mapping.split_once(':') {
            Some((field, column)) => (field.trim().to_owned(), column.trim().to_owned()),
            None => (mapping.clone(), mapping),
        };
        Self {
            origin_column: properties::trimmed(config, EXTRACT_JSON_ORIGIN_COLUMN)
                .unwrap_or_default(),
            path: JsonPath::parse(&path),
            target_column,
            overwrite: properties::boolean(config, EXTRACT_JSON_OVERWRITE, false),
            exclusive: properties::boolean(config, EXTRACT_JSON_EXCLUSIVE, false),
        }
    }

    /// Whether both the origin column and a target column are configured.
    pub fn is_enabled(&self) -> bool {
        !self.origin_column.is_empty() && !self.target_column.is_empty()
    }

    /// The origin column holding the JSON document.
    pub fn origin_column(&self) -> &str {
        &self.origin_column
    }

    /// The property inside the document.
    pub const fn path(&self) -> &JsonPath {
        &self.path
    }

    /// The target column the extracted value is written to.
    pub fn target_column(&self) -> &str {
        &self.target_column
    }

    /// Whether an already-populated target column is overwritten (`FEA-032`).
    pub const fn overwrites(&self) -> bool {
        self.overwrite
    }

    /// Whether the run is narrowed to the extract column alone (`FEA-033`).
    pub const fn is_exclusive(&self) -> bool {
        self.exclusive
    }

    /// Validates the configuration against both schemas (`FEA-030`).
    pub fn validate(&self, schema: &FeatureSchema) -> Vec<Diagnostic> {
        let configured = usize::from(!self.origin_column.is_empty())
            + usize::from(!self.target_column.is_empty());
        if configured == 0 {
            return Vec::new();
        }
        if configured == 1 {
            return vec![diagnostic::config_error(
                "extract JSON requires both an origin column and a property mapping, or neither",
            )
            .with_rule("FEA-030")];
        }

        let mut findings = Vec::new();
        match schema.origin.column(&self.origin_column) {
            None => findings.push(
                diagnostic::schema_error(format!(
                    "extract-JSON origin column `{}` is not on the origin table {}",
                    self.origin_column,
                    schema.origin.table()
                ))
                .with_rule("FEA-030"),
            ),
            Some(column)
                if !matches!(column.cql_type(), CqlTypeInfo::Text | CqlTypeInfo::Ascii) =>
            {
                findings.push(
                    diagnostic::schema_error(format!(
                        "extract-JSON origin column `{}` is {}, not a text column",
                        self.origin_column,
                        column.cql_type()
                    ))
                    .with_rule("FEA-030"),
                );
            }
            Some(_) => {}
        }
        if schema.target.column(&self.target_column).is_none() {
            findings.push(
                diagnostic::schema_error(format!(
                    "extract-JSON target column `{}` is not on the target table {}",
                    self.target_column,
                    schema.target.table()
                ))
                .with_rule("FEA-031"),
            );
        }
        findings
    }

    /// The target columns a run writes when `exclusive` is set (`FEA-033`).
    ///
    /// The primary key always survives — without it there is no row to write — and the extract
    /// column is the only other column retained. Matching is by exact name; see the module
    /// documentation for why that differs from Java.
    pub fn exclusive_target_columns<'a>(&self, target: &'a TableFacts) -> Vec<&'a str> {
        if !self.exclusive || !self.is_enabled() {
            return target
                .columns()
                .iter()
                .map(crate::schema::ColumnFacts::name)
                .collect();
        }
        target
            .columns()
            .iter()
            .filter(|column| column.is_key() || column.name() == self.target_column)
            .map(crate::schema::ColumnFacts::name)
            .collect()
    }

    /// The origin columns a run reads when `exclusive` is set (`FEA-033`).
    ///
    /// The mirror of [`ExtractJson::exclusive_target_columns`]: reading the whole origin row to use
    /// one column of it would cost bandwidth the setting exists to save.
    pub fn exclusive_origin_columns<'a>(&self, origin: &'a TableFacts) -> Vec<&'a str> {
        if !self.exclusive || !self.is_enabled() {
            return origin
                .columns()
                .iter()
                .map(crate::schema::ColumnFacts::name)
                .collect();
        }
        origin
            .columns()
            .iter()
            .filter(|column| column.is_key() || column.name() == self.origin_column)
            .map(crate::schema::ColumnFacts::name)
            .collect()
    }

    /// Resolves the column positions and the target type (`FEA-031`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`] when either column is missing — the conditions
    /// [`ExtractJson::validate`] reports as diagnostics.
    pub fn resolve(&self, schema: &FeatureSchema) -> Result<ExtractJsonPlan, CdmError> {
        let origin_index = schema
            .origin
            .index_of(&self.origin_column)
            .ok_or_else(|| missing(&self.origin_column, &schema.origin.table().to_string()))?;
        let target = schema
            .target
            .column(&self.target_column)
            .ok_or_else(|| missing(&self.target_column, &schema.target.table().to_string()))?;
        Ok(ExtractJsonPlan {
            origin_index,
            path: self.path.clone(),
            target_column: self.target_column.clone(),
            target_type: target.cql_type().clone(),
            overwrite: self.overwrite,
        })
    }
}

fn missing(column: &str, table: &str) -> CdmError {
    CdmError::new(
        ErrorKind::SchemaMismatch,
        format!("extract-JSON column `{column}` is not on table {table}"),
    )
    .with_context(|c| c.with_column(column.to_owned()))
}

/// The resolved feature: positions, path and target type, ready for the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractJsonPlan {
    origin_index: usize,
    path: JsonPath,
    target_column: String,
    target_type: CqlTypeInfo,
    overwrite: bool,
}

impl ExtractJsonPlan {
    /// The position of the document column in the origin projection.
    pub const fn origin_index(&self) -> usize {
        self.origin_index
    }

    /// The target column the extracted value is written to.
    pub fn target_column(&self) -> &str {
        &self.target_column
    }

    /// Extracts the mapped property from one document cell (`FEA-030`, `FEA-031`, `FEA-035`).
    ///
    /// Returns `None` when there is nothing to write: a null or blank document, or a path that
    /// addresses nothing. `None` is not the same as a JSON `null`, which *is* written and therefore
    /// comes back as a null [`RawCell`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the document does not parse (`FEA-034`) or the
    /// value does not fit the target column's type.
    pub fn extract(&self, cell: &RawCell) -> Result<Option<RawCell>, CdmError> {
        let Some(bytes) = cell.bytes() else {
            return Ok(None);
        };
        let text = std::str::from_utf8(bytes).map_err(|e| {
            CdmError::new(
                ErrorKind::TypeConversion,
                "extract-JSON origin column is not valid UTF-8 text",
            )
            .with_context(|c| c.with_column(self.target_column.clone()))
            .with_source(e)
        })?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        let document: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            CdmError::new(
                ErrorKind::TypeConversion,
                format!(
                    "extract-JSON origin column does not contain a JSON document: {} at line {}, \
                     column {}",
                    e.classify_as_str(),
                    e.line(),
                    e.column()
                ),
            )
            .with_context(|c| c.with_column(self.target_column.clone()))
        })?;
        self.path
            .resolve(&document)
            .map(|value| encode_json(value, &self.target_type))
            .transpose()
    }

    /// Extracts from a record, tagging any failure with the row's primary key (`FEA-034`).
    ///
    /// # Errors
    ///
    /// As [`ExtractJsonPlan::extract`], plus [`ErrorKind::Internal`] if the row is shorter than the
    /// projection the plan was built from. The primary key rides on the error's context so that the
    /// engine can log *which* row had the malformed document without the feature logging anything
    /// itself (`SEC-002` forbids logging the row).
    pub fn extract_record(&self, record: &Record) -> Result<Option<RawCell>, CdmError> {
        self.extract(record.origin_cell(self.origin_index)?)
            .map_err(|e| e.with_context(|c| c.with_primary_key(record.key().clone())))
    }

    /// Whether the extracted value should replace what the target already holds (`FEA-032`).
    ///
    /// With `overwrite = false` a populated target column is left exactly as it is — the feature is
    /// then a backfill, not a rewrite, and re-running it is free of surprises.
    pub fn writes_over(&self, current: Option<&RawCell>) -> bool {
        if self.overwrite {
            return true;
        }
        match current {
            None => true,
            Some(cell) => cell.is_null(),
        }
    }
}

/// Jackson-style classification of a `serde_json` error, for the message of `FEA-034`.
trait ClassifyExt {
    fn classify_as_str(&self) -> &'static str;
}

impl ClassifyExt for serde_json::Error {
    fn classify_as_str(&self) -> &'static str {
        match self.classify() {
            serde_json::error::Category::Io => "I/O failure",
            serde_json::error::Category::Syntax => "malformed JSON",
            serde_json::error::Category::Data => "unexpected JSON shape",
            serde_json::error::Category::Eof => "truncated JSON",
        }
    }
}

impl Plugin for ExtractJsonPlan {
    fn name(&self) -> &'static str {
        "extract-json"
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl FeaturePlugin for ExtractJsonPlan {
    fn is_enabled(&self, _config: &EffectiveConfig) -> bool {
        true
    }

    /// The plan exists only because [`ExtractJson::validate`] already passed.
    fn validate(&self, _config: &EffectiveConfig, _schema: &SchemaPair) -> Vec<Diagnostic> {
        Vec::new()
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
    use cdm_core::{PrimaryKey, Row, TableRef};

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn schema() -> FeatureSchema {
        let origin = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("doc", "text"), ("other", "text")],
            ),
            &["id"],
        )
        .unwrap();
        let target = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[
                    ("id", "int"),
                    ("city", "text"),
                    ("age", "int"),
                    ("other", "text"),
                ],
            ),
            &["id"],
        )
        .unwrap();
        FeatureSchema::new(origin, target)
    }

    fn feature(mapping: &str) -> ExtractJson {
        ExtractJson::load(&config(&[
            ("feature.extract_json.origin_column", "doc"),
            ("feature.extract_json.property_mapping", mapping),
        ]))
    }

    fn document(json: &str) -> RawCell {
        RawCell::new(json.as_bytes().to_vec())
    }

    #[test]
    fn fea_030_the_mapping_is_field_to_column_or_a_bare_name() {
        let mapped = feature("city:city");
        assert!(mapped.is_enabled());
        assert_eq!(mapped.path().as_str(), "city");
        assert_eq!(mapped.target_column(), "city");
        assert_eq!(mapped.origin_column(), "doc");

        let bare = ExtractJson::load(&config(&[
            ("spark.cdm.feature.extractJson.originColumn", "doc"),
            ("spark.cdm.feature.extractJson.propertyMapping", "other"),
        ]));
        assert_eq!(bare.path().as_str(), "other");
        assert_eq!(bare.target_column(), "other");
        assert!(bare.validate(&schema()).is_empty());
    }

    #[test]
    fn fea_030_the_origin_column_must_exist_and_hold_text() {
        assert!(feature("city:city").validate(&schema()).is_empty());

        let not_text = TableFacts::from_view(
            &table_view(TableRef::new("ks", "src"), &[("id", "int"), ("doc", "int")]),
            &["id"],
        )
        .unwrap();
        let findings =
            feature("city:city").validate(&FeatureSchema::new(not_text, schema().target));
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("not a text column"));

        let missing_target = feature("city:nope").validate(&schema());
        assert_eq!(missing_target.len(), 1);
        assert_eq!(missing_target[0].rule.as_deref(), Some("FEA-031"));

        let half = ExtractJson::load(&config(&[("feature.extract_json.origin_column", "doc")]));
        assert!(!half.is_enabled());
        assert_eq!(half.validate(&schema()).len(), 1);
        assert!(ExtractJson::load(&EffectiveConfig::new())
            .validate(&schema())
            .is_empty());
    }

    #[test]
    fn fea_031_the_extracted_value_is_encoded_for_the_target_column() {
        let plan = feature("city:city").resolve(&schema()).unwrap();
        assert_eq!(plan.origin_index(), 1);
        assert_eq!(plan.target_column(), "city");
        assert_eq!(
            plan.extract(&document(r#"{"city":"Paris"}"#)).unwrap(),
            Some(RawCell::new(b"Paris".to_vec()))
        );

        let typed = feature("age:age").resolve(&schema()).unwrap();
        assert_eq!(
            typed.extract(&document(r#"{"age":42}"#)).unwrap(),
            Some(RawCell::new(42_i32.to_be_bytes().to_vec()))
        );
        assert_eq!(
            typed.extract(&document(r#"{"age":null}"#)).unwrap(),
            Some(RawCell::NULL),
            "an explicit JSON null is a value, and is written"
        );
        assert_eq!(
            typed.extract(&document(r#"{"other":1}"#)).unwrap(),
            None,
            "an absent property writes nothing at all"
        );
        assert_eq!(typed.extract(&RawCell::NULL).unwrap(), None);
        assert_eq!(typed.extract(&document("   ")).unwrap(), None);
        assert!(feature("city:nope").resolve(&schema()).is_err());
    }

    #[test]
    fn fea_032_overwrite_false_leaves_a_populated_target_column_untouched() {
        let plan = feature("city:city").resolve(&schema()).unwrap();
        assert!(plan.writes_over(None));
        assert!(plan.writes_over(Some(&RawCell::NULL)));
        assert!(!plan.writes_over(Some(&RawCell::new(b"Lyon".to_vec()))));

        let overwriting = ExtractJson::load(&config(&[
            ("feature.extract_json.origin_column", "doc"),
            ("feature.extract_json.property_mapping", "city:city"),
            ("feature.extract_json.overwrite", "true"),
        ]))
        .resolve(&schema())
        .unwrap();
        assert!(overwriting.writes_over(Some(&RawCell::new(b"Lyon".to_vec()))));
    }

    #[test]
    fn fea_033_exclusive_restricts_the_non_key_columns_to_the_extract_column() {
        let schema = schema();
        let exclusive = ExtractJson::load(&config(&[
            ("feature.extract_json.origin_column", "doc"),
            ("feature.extract_json.property_mapping", "city:city"),
            ("feature.extract_json.exclusive", "true"),
        ]));
        assert!(exclusive.is_exclusive());
        assert_eq!(
            exclusive.exclusive_target_columns(&schema.target),
            ["id", "city"]
        );
        assert_eq!(
            exclusive.exclusive_origin_columns(&schema.origin),
            ["id", "doc"]
        );

        let inclusive = feature("city:city");
        assert!(!inclusive.is_exclusive());
        assert_eq!(
            inclusive.exclusive_target_columns(&schema.target),
            ["id", "city", "age", "other"]
        );
        assert_eq!(
            inclusive.exclusive_origin_columns(&schema.origin),
            ["id", "doc", "other"]
        );
    }

    #[test]
    fn fea_033_exclusive_matching_is_exact_rather_than_a_suffix() {
        // Java filters with `endsWith`, so `city` would also retain `oldcity`.
        let target = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[("id", "int"), ("city", "text"), ("oldcity", "text")],
            ),
            &["id"],
        )
        .unwrap();
        let exclusive = ExtractJson::load(&config(&[
            ("feature.extract_json.origin_column", "doc"),
            ("feature.extract_json.property_mapping", "city:city"),
            ("feature.extract_json.exclusive", "true"),
        ]));
        assert_eq!(exclusive.exclusive_target_columns(&target), ["id", "city"]);
    }

    #[test]
    fn fea_034_malformed_json_is_a_record_level_error_carrying_the_primary_key() {
        let plan = feature("city:city").resolve(&schema()).unwrap();
        let error = plan.extract(&document("{not json")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);
        assert!(error.message().contains("malformed JSON"));

        let key = PrimaryKey::new(vec![RawCell::new(vec![0, 0, 0, 7])]);
        let record = Record::new(
            key.clone(),
            Row::new(vec![
                RawCell::new(vec![0, 0, 0, 7]),
                document("{not json"),
                RawCell::NULL,
            ]),
        );
        let error = plan.extract_record(&record).unwrap_err();
        assert_eq!(error.context().primary_key.as_ref(), Some(&key));

        let short = Record::new(PrimaryKey::default(), Row::new(vec![RawCell::NULL]));
        assert_eq!(
            plan.extract_record(&short).unwrap_err().kind(),
            ErrorKind::Internal
        );
        assert!(plan.extract(&RawCell::new(vec![0xff, 0xfe])).is_err());
    }

    #[test]
    fn fea_035_a_mapping_may_be_a_json_pointer() {
        let plan = feature("/address/city:city").resolve(&schema()).unwrap();
        assert!(matches!(plan.path, JsonPath::Pointer(_)));
        assert_eq!(
            plan.extract(&document(r#"{"address":{"city":"Oslo"}}"#))
                .unwrap(),
            Some(RawCell::new(b"Oslo".to_vec()))
        );

        let indexed = feature("/tags/1:city").resolve(&schema()).unwrap();
        assert_eq!(
            indexed.extract(&document(r#"{"tags":["a","b"]}"#)).unwrap(),
            Some(RawCell::new(b"b".to_vec()))
        );
        assert_eq!(indexed.extract(&document(r#"{"tags":[]}"#)).unwrap(), None);
        assert_eq!(JsonPath::parse("a/b"), JsonPath::Field("a/b".to_owned()));
    }

    #[test]
    fn fea_030_the_resolved_plan_registers_as_a_plugin() {
        let plan = feature("city:city").resolve(&schema()).unwrap();
        assert_eq!(Plugin::name(&plan), "extract-json");
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
}
