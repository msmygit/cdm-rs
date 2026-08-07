//! Comparing one origin record against the target row fetched for it (`VAL-005`, `VAL-006`,
//! `VAL-009`, `VAL-011`).
//!
//! # The direction of the conversion is not symmetric
//!
//! `VAL-005` requires the **target** value to be converted into the **origin's** type space, and
//! the origin value to be compared exactly as it was read. That asymmetry is Java's
//! (`DiffJobSession.isDifferent` calls `getAndConvertData` on the target row and plain `getData` on
//! the origin row) and it is load-bearing: a migration that writes `int` into `text` must validate
//! by parsing the target's `"10"` back to `10`, not by re-formatting the origin's `10` and hoping
//! the two renderings agree. Comparing in the origin's space also means the conversion under test
//! is the inverse of the one the migration performed, so a lossy codec shows up as a mismatch
//! rather than cancelling itself out.
//!
//! Everything about that is resolved once, at startup: one [`ConversionPlan`] per target column,
//! built by [`ComparisonPlan::resolve`]. Per row the comparator indexes into a vector
//! (`ARCHITECTURE.md` §5.5).
//!
//! # Equality is byte equality, after conversion
//!
//! Two cells are equal when their serialised bytes are equal *once the target's have been converted
//! into the origin's type*. That is not a weakening of Java's `Object.equals`: Cassandra's wire form
//! is canonical for every type where equality is defined — a `set` is serialised in sorted order, a
//! `map` in key order — so byte equality and value equality coincide, and the cases where they
//! would not (a `decimal`'s scale, a `list`'s order) are exactly the cases where Java's `equals`
//! also reports a difference.
//!
//! The consequence the caller cares about: two values that differ *in wire representation* but
//! agree after conversion are **not** a discrepancy, because the conversion runs first.
//!
//! # Values are never rendered (`SEC-002`, `VAL-017`)
//!
//! Java's mismatch detail contains the differing values. `SEC-002` forbids logging row values by
//! default, and `ERR-005` already resolved exactly this conflict in `SEC-002`'s favour by carrying
//! the primary key instead of the value. `VAL-017` applies the same resolution here: the detail
//! keeps Java's shape, so existing log scrapers keep matching, but every value position renders as
//! `<redacted>`. Null-ness is metadata rather than content and is reported, because "the target is
//! null" and "the target differs" call for different actions.
//!
//! A [`Mismatch`] does *carry* the two cells, because the discrepancy report of `VAL-013` is
//! entitled to them and re-reading the row to get them back would be absurd. They are carried
//! `pub(crate)`, reachable only from [`report`](super::report), which applies
//! `validate.report.redact_values` while building the record — the same discipline the event bus
//! uses for primary keys. There is no public accessor, so [`Mismatch::detail`] and the diff log
//! remain the only things this type renders in the clear, and neither of them renders a value.
//! Holding the cells costs nothing measurable: a [`RawCell`] is a refcounted `Bytes`, and only
//! differing columns of differing rows are kept at all.

use cdm_codec::{ConversionPlan, CqlTypeInfo, Planner};
use cdm_core::{CdmError, ErrorKind, RawCell, Record, Row, Side};
use cdm_cql::schema::ColumnMeta;
use cdm_cql::statement::{ColumnMapping, TargetSource};
use cdm_feature::ExtractJsonPlan;

/// What one value position renders as (`SEC-002`, `VAL-017`).
pub const REDACTED: &str = "<redacted>";

/// How the target column's value is obtained and whether it is compared at all.
#[derive(Debug, Clone)]
enum Rule {
    /// A constant column. `VAL-005` excludes it: its value came from configuration, not from the
    /// origin, so there is nothing on the origin to disagree with.
    Constant,
    /// An ordinary mapped column, at this position in the origin projection.
    Mapped(usize),
    /// The extract-JSON column (`FEA-030`), whose origin side is a property of the document in the
    /// origin column rather than a column of its own (`VAL-011`).
    ExtractJson,
    /// Nothing on the origin supplies this column, so the origin side is `null` (`SCH-006`).
    Unsourced,
    /// A column cdm-rs cannot obtain an origin value for from the record alone — the two halves of
    /// an exploded map entry (`FEA-020`). Reported through `VAL-009`'s exception form, which is
    /// what Java does here too, by accident.
    Unobtainable(&'static str),
}

/// One target column, resolved for comparison.
#[derive(Debug, Clone)]
struct ColumnCompare {
    name: String,
    target_index: usize,
    rule: Rule,
    /// Converts the target's value into the origin's type space (`VAL-005`).
    plan: ConversionPlan,
}

/// Every target column, resolved once (`VAL-005`).
///
/// Built at startup from the same [`ColumnMapping`] the statements are built from, so the
/// comparison and the write can never disagree about which origin column feeds which target one.
#[derive(Debug, Clone)]
pub struct ComparisonPlan {
    columns: Vec<ColumnCompare>,
    extract_json: Option<ExtractJsonPlan>,
    extract_json_overwrites: bool,
    keys_only: bool,
}

impl ComparisonPlan {
    /// Resolves the plan.
    ///
    /// `extract_json` is `Some` when `feature.extract_json` is active; `overwrites` is that
    /// feature's `overwrite` setting, which `VAL-011` turns into "skip an already-populated target
    /// column rather than compare it".
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] when a column's declared type does not parse. That is a
    /// disagreement between cdm-rs and `system_schema` rather than bad data, and it must stop the
    /// run before a single row is misreported as a mismatch.
    pub fn resolve(
        mapping: &ColumnMapping,
        planner: &Planner,
        extract_json: Option<ExtractJsonPlan>,
        extract_json_overwrites: bool,
    ) -> Result<Self, CdmError> {
        let mut columns = Vec::with_capacity(mapping.target_columns().len());
        for (target_index, column) in mapping.target_columns().iter().enumerate() {
            let target_type = parse_type(column, Side::Target)?;
            let source = mapping
                .source(target_index)
                .cloned()
                .unwrap_or(TargetSource::Absent);
            let (rule, plan) = match &source {
                TargetSource::Constant(_) => (Rule::Constant, ConversionPlan::Passthrough),
                TargetSource::Origin(origin_index) => {
                    let origin_column =
                        mapping.origin_columns().get(*origin_index).ok_or_else(|| {
                            CdmError::new(
                                ErrorKind::SchemaMismatch,
                                format!(
                                    "the column mapping refers to origin column {origin_index}, \
                                     but the origin projection has only {} columns",
                                    mapping.origin_columns().len()
                                ),
                            )
                        })?;
                    let origin_type = parse_type(origin_column, Side::Origin)?;
                    // VAL-005: target into the origin's space, which is the reverse of the
                    // direction the migration converted in.
                    (
                        Rule::Mapped(*origin_index),
                        planner.plan_types(&target_type, &origin_type),
                    )
                }
                // The extracted property is encoded against the *target* type by `FEA-031`, so
                // both sides are already in the same space and nothing needs converting.
                TargetSource::ExtractJson(_) => (Rule::ExtractJson, ConversionPlan::Passthrough),
                TargetSource::ExplodeKey => (
                    Rule::Unobtainable("the explode-map key is not carried on the record"),
                    ConversionPlan::Passthrough,
                ),
                TargetSource::ExplodeValue => (
                    Rule::Unobtainable("the explode-map value is not carried on the record"),
                    ConversionPlan::Passthrough,
                ),
                TargetSource::Absent => (Rule::Unsourced, ConversionPlan::Passthrough),
            };
            columns.push(ColumnCompare {
                name: column.name.clone(),
                target_index,
                rule,
                plan,
            });
        }
        Ok(Self {
            columns,
            extract_json,
            extract_json_overwrites,
            keys_only: false,
        })
    }

    /// Compares existence only (`VAL-015`, `validate.keys_only`).
    ///
    /// The pre-flight run: every column plan is still resolved, because a plan that cannot be
    /// built is a configuration error and must surface at startup whether or not this run intends
    /// to use it, but no column is compared and no value is converted. What remains is one target
    /// lookup per origin row, which is the part of a validation that cannot be made cheaper.
    ///
    /// A run in this mode structurally cannot report `MISMATCH`, so its `VALID` count means "the
    /// row is there", not "the row is right".
    #[must_use]
    pub const fn with_keys_only(mut self, keys_only: bool) -> Self {
        self.keys_only = keys_only;
        self
    }

    /// Whether this plan compares existence only (`VAL-015`).
    #[must_use]
    pub const fn is_keys_only(&self) -> bool {
        self.keys_only
    }

    /// How many target columns the plan compares, constants included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether the plan has no columns at all, which cannot happen for an introspected table.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Compares one record against the target row fetched for it (`VAL-005`, `VAL-008`).
    ///
    /// `target` is `None` when the target has no row for the record's key, which is `VAL-002`'s
    /// missing case and is reported without comparing anything.
    #[must_use]
    pub fn compare(&self, record: &Record, target: Option<&Row>) -> Comparison {
        let Some(target) = target else {
            return Comparison::Missing;
        };
        // VAL-015: the target has a row, and that was the whole question.
        if self.keys_only {
            return Comparison::Valid;
        }
        let mut differences = Vec::new();
        for column in &self.columns {
            if let Some(difference) = self.compare_column(column, record, target) {
                differences.push(difference);
            }
        }
        if differences.is_empty() {
            Comparison::Valid
        } else {
            Comparison::Mismatch(Mismatch { differences })
        }
    }

    /// One column's contribution, or `None` when the two sides agree.
    fn compare_column(
        &self,
        column: &ColumnCompare,
        record: &Record,
        target: &Row,
    ) -> Option<ColumnDifference> {
        let target_cell = target.get(column.target_index);
        match &column.rule {
            // VAL-005: a constant column is never compared.
            Rule::Constant => None,
            Rule::Unobtainable(why) => Some(ColumnDifference {
                column: column.name.clone(),
                origin: RawCell::NULL,
                target: cell(target_cell),
                kind: DifferenceKind::Error {
                    message: (*why).to_owned(),
                    target_index: column.target_index,
                    origin_index: -1,
                },
            }),
            Rule::Unsourced => Self::compare_values(column, None, target_cell, -1),
            Rule::Mapped(origin_index) => {
                let origin_cell = record.origin().get(*origin_index);
                let origin_index = i64::try_from(*origin_index).unwrap_or(-1);
                if origin_cell.is_none() {
                    // The projection and the row disagree: a plan bug, not bad data. `VAL-009`
                    // says to capture it into the detail rather than fail the range.
                    return Some(ColumnDifference {
                        column: column.name.clone(),
                        origin: RawCell::NULL,
                        target: cell(target_cell),
                        kind: DifferenceKind::Error {
                            message: "the origin row is narrower than the projection".to_owned(),
                            target_index: column.target_index,
                            origin_index,
                        },
                    });
                }
                Self::compare_values(column, origin_cell, target_cell, origin_index)
            }
            Rule::ExtractJson => self.compare_extracted(column, record, target_cell),
        }
    }

    /// The extract-JSON column (`VAL-011`).
    fn compare_extracted(
        &self,
        column: &ColumnCompare,
        record: &Record,
        target_cell: Option<&RawCell>,
    ) -> Option<ColumnDifference> {
        let populated = target_cell.is_some_and(|cell| !cell.is_null());
        // VAL-011: with `overwrite = false` the migration left a populated target column alone, so
        // validating it against the origin document would report a difference the run deliberately
        // created. Java skips the column entirely here, and so do we.
        if !self.extract_json_overwrites && populated {
            return None;
        }
        let plan = self.extract_json.as_ref()?;
        match plan.extract_record(record) {
            Ok(extracted) => {
                let owned = extracted.unwrap_or(RawCell::NULL);
                Self::compare_values(
                    column,
                    Some(&owned),
                    target_cell,
                    i64::try_from(plan.origin_index()).unwrap_or(-1),
                )
            }
            // FEA-034 counts a malformed document as a record-level error; inside validate it is a
            // per-column comparison error, which `VAL-009` puts in the detail.
            Err(error) => Some(ColumnDifference {
                column: column.name.clone(),
                origin: RawCell::NULL,
                target: cell(target_cell),
                kind: DifferenceKind::Error {
                    message: error.to_string(),
                    target_index: column.target_index,
                    origin_index: i64::try_from(plan.origin_index()).unwrap_or(-1),
                },
            }),
        }
    }

    /// The comparison proper, once both sides have been located.
    fn compare_values(
        column: &ColumnCompare,
        origin: Option<&RawCell>,
        target: Option<&RawCell>,
        origin_index: i64,
    ) -> Option<ColumnDifference> {
        let origin_null = origin.is_none_or(RawCell::is_null);
        let target_null = target.is_none_or(RawCell::is_null);

        // Both null is equality, in Java and here. It is also the only case where the conversion is
        // not attempted, since there is nothing to convert.
        if origin_null && target_null {
            return None;
        }
        if origin_null {
            return Some(ColumnDifference {
                column: column.name.clone(),
                origin: RawCell::NULL,
                target: cell(target),
                kind: DifferenceKind::OriginNull,
            });
        }

        let Some(target_cell) = target.filter(|value| !value.is_null()) else {
            return Some(ColumnDifference {
                column: column.name.clone(),
                origin: cell(origin),
                target: RawCell::NULL,
                kind: DifferenceKind::Value {
                    target_is_null: true,
                },
            });
        };

        // VAL-005: into the origin's type space before anything is compared.
        let converted = if column.plan.is_identity() {
            target_cell.clone()
        } else {
            match column.plan.apply(target_cell) {
                Ok(converted) => converted,
                // VAL-009: a conversion that throws is detail, not a failed range.
                Err(error) => {
                    return Some(ColumnDifference {
                        column: column.name.clone(),
                        origin: cell(origin),
                        target: target_cell.clone(),
                        kind: DifferenceKind::Error {
                            message: error.to_string(),
                            target_index: column.target_index,
                            origin_index,
                        },
                    })
                }
            }
        };

        if origin.is_some_and(|value| value.bytes() == converted.bytes()) {
            None
        } else {
            Some(ColumnDifference {
                column: column.name.clone(),
                origin: cell(origin),
                // The target as it was *read*, not as it was converted: a report that showed the
                // converted form would be showing a value that exists nowhere, and the operator
                // who goes and looks at the row would not find it there.
                target: target_cell.clone(),
                kind: DifferenceKind::Value {
                    target_is_null: false,
                },
            })
        }
    }
}

/// A cell, or `NULL` when the row does not have one at that position.
fn cell(value: Option<&RawCell>) -> RawCell {
    value.cloned().unwrap_or(RawCell::NULL)
}

/// What comparing one record concluded (`VAL-002`, `VAL-006`, `VAL-008`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    /// Every compared column agreed. `VAL-008`: `VALID`.
    Valid,
    /// The target has no row for this key. `VAL-002`: `MISSING`.
    Missing,
    /// At least one column differed, or could not be compared. `VAL-006`: `MISMATCH`.
    Mismatch(Mismatch),
}

impl Comparison {
    /// Whether this outcome is a discrepancy, which is what `VAL-016` resolves a status from.
    #[must_use]
    pub const fn is_discrepancy(&self) -> bool {
        !matches!(self, Self::Valid)
    }
}

/// Every column that differed, and the detail string `VAL-006` logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    differences: Vec<ColumnDifference>,
}

impl Mismatch {
    /// The differing columns, in target-column order.
    ///
    /// Order is a deliberate improvement on Java, which iterates the columns in a `parallel()`
    /// stream over a shared `StringBuffer` and therefore emits them in whatever order the
    /// fork-join pool produced. Two runs over the same data give the same detail here, which is
    /// what makes two diff logs comparable at all.
    #[must_use]
    pub fn columns(&self) -> Vec<String> {
        self.differences
            .iter()
            .map(|difference| difference.column.clone())
            .collect()
    }

    /// How many columns differed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.differences.len()
    }

    /// Whether nothing differed, which a [`Mismatch`] is never constructed for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.differences.is_empty()
    }

    /// The `VAL-006` detail string, with every value position redacted (`VAL-017`).
    #[must_use]
    pub fn detail(&self) -> String {
        let mut detail = String::new();
        for difference in &self.differences {
            difference.render(&mut detail);
        }
        detail
    }

    /// The differences with their values attached, for the report of `VAL-013`.
    ///
    /// Crate-visible on purpose: these are row values, and the only thing allowed to see them is
    /// the report writer, which redacts before it records. See the module documentation.
    pub(crate) fn differences(&self) -> &[ColumnDifference] {
        &self.differences
    }
}

/// One column's disagreement, with the two cells that disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnDifference {
    column: String,
    origin: RawCell,
    target: RawCell,
    kind: DifferenceKind,
}

impl ColumnDifference {
    /// The target column's name.
    pub(crate) fn column(&self) -> &str {
        &self.column
    }

    /// The origin's cell, `NULL` when the origin had nothing there.
    pub(crate) const fn origin(&self) -> &RawCell {
        &self.origin
    }

    /// The target's cell as it was read, `NULL` when the target had nothing there.
    pub(crate) const fn target(&self) -> &RawCell {
        &self.target
    }

    /// Why the column could not be compared at all (`VAL-009`), when that is what happened.
    pub(crate) fn error(&self) -> Option<&str> {
        match &self.kind {
            DifferenceKind::Error { message, .. } => Some(message),
            DifferenceKind::Value { .. } | DifferenceKind::OriginNull => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DifferenceKind {
    /// The two values differ. `target_is_null` distinguishes "the target has something else" from
    /// "the target has nothing", which call for different actions.
    Value { target_is_null: bool },
    /// The origin has no value and the target does — Java's second message form.
    OriginNull,
    /// The column could not be compared at all (`VAL-009`).
    Error {
        message: String,
        target_index: usize,
        origin_index: i64,
    },
}

impl ColumnDifference {
    /// Appends this column's fragment to the detail, in Java's shape (`VAL-006`, `VAL-009`).
    fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        let name = &self.column;
        // `write!` into a `String` is infallible; the result is discarded rather than unwrapped so
        // that `ERR-004` holds without a targeted allow.
        let _ = match &self.kind {
            DifferenceKind::Value {
                target_is_null: false,
            } => write!(
                out,
                "Target column:{name}-origin[{REDACTED}]-target[{REDACTED}]; "
            ),
            DifferenceKind::Value {
                target_is_null: true,
            } => write!(
                out,
                "Target column:{name}-origin[{REDACTED}]-target[null]; "
            ),
            DifferenceKind::OriginNull => {
                write!(
                    out,
                    "Target column:{name} origin is null, target is {REDACTED}; "
                )
            }
            DifferenceKind::Error {
                message,
                target_index,
                origin_index,
            } => write!(
                out,
                "Target column:{name} Exception {message} targetIndex:{target_index} \
                 originIndex:{origin_index}; "
            ),
        };
    }
}

/// Parses a column's declared type, naming the side when it does not parse.
fn parse_type(column: &ColumnMeta, side: Side) -> Result<CqlTypeInfo, CdmError> {
    CqlTypeInfo::parse(&column.cql_type).map_err(|error| {
        CdmError::new(
            ErrorKind::SchemaMismatch,
            format!(
                "cannot parse the {side} type `{}` of column `{}` for comparison: {error}",
                column.cql_type, column.name
            ),
        )
        .with_context(|c| c.with_side(side).with_column(column.name.clone()))
    })
}
