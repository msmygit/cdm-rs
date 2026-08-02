//! The conversion planner (`CDC-010`..`CDC-016`).
//!
//! A [`ConversionPlan`] is computed **once per column pair at startup** and then applied per row.
//! Resolving the plan is the expensive part — parsing types, searching the codec registry,
//! recursing into element types — and none of it happens again while data is moving
//! (`ARCHITECTURE.md` §5.5).
//!
//! ```text
//! identical types            -> Passthrough      (raw bytes, MIG-040)
//! assignable representations -> Passthrough
//! registered codec           -> Codec
//! same-kind collection       -> List/Set/Map/Tuple/Vector, recursively
//! UDT to UDT                 -> Udt, field by field
//! anything else              -> Unsupported      (identity + one warning at startup)
//! ```
//!
//! # Improvements over Java, and how to turn them off
//!
//! Two behaviours differ from Java CDM deliberately, both restorable with `--compat-java`
//! ([`PlannerOptions::compat_java`]):
//!
//! * **`CDC-014`** — UDT fields are matched by **name** when the two field-name sets are equal,
//!   falling back to position. Java matches strictly by position and converts each field by
//!   round-tripping it through `TypeCodec.format`/`parse`, i.e. through a string, which loses
//!   precision and silently mis-assigns values when the field order differs.
//! * **`CDC-015`** — tuple elements are converted. Java's `CqlConversion.Type` has a
//!   `// TODO: add TUPLE to this list` comment and leaves tuples entirely unconverted.

use std::fmt;
use std::sync::Arc;

use cdm_core::{CdmError, Diagnostic, ErrorKind, RawCell};

use crate::codec::{CodecRegistry, Converter};
use crate::types::CqlTypeInfo;
use crate::wire::{conversion_error, write_element, Reader};

/// How one column's values are converted (`CDC-010`).
#[derive(Debug, Clone)]
pub enum ConversionPlan {
    /// Identical, or directly assignable, types: the raw bytes are moved unchanged (`CDC-011`,
    /// `MIG-040`).
    Passthrough,
    /// A registered codec converts the value.
    Codec(Arc<dyn Converter>),
    /// A UDT converted field by field (`CDC-013`, `CDC-014`).
    Udt {
        /// One entry per **target** field, naming the origin field it takes its value from and
        /// how that value is converted.
        fields: Vec<UdtFieldPlan>,
    },
    /// A list, converted element-wise (`CDC-012`).
    List(Box<ConversionPlan>),
    /// A set, converted element-wise (`CDC-012`).
    Set(Box<ConversionPlan>),
    /// A map, converted key-wise and value-wise (`CDC-012`).
    Map {
        /// How keys are converted.
        key: Box<ConversionPlan>,
        /// How values are converted.
        value: Box<ConversionPlan>,
    },
    /// A tuple, converted positionally (`CDC-015`).
    Tuple(Vec<ConversionPlan>),
    /// A vector, converted element-wise (`CDC-004`).
    Vector {
        /// How each element is converted.
        element: Box<ConversionPlan>,
        /// The number of elements, which is part of the type.
        dimensions: usize,
        /// The serialised width of one **origin** element, which is how the contiguous array is
        /// split.
        origin_width: usize,
    },
    /// No conversion is known. The value passes through unchanged and the column is warned about
    /// once, at startup (`CDC-016`).
    Unsupported {
        /// The origin type.
        origin: Box<CqlTypeInfo>,
        /// The target type.
        target: Box<CqlTypeInfo>,
    },
}

/// How one target UDT field is populated (`CDC-013`, `CDC-014`).
#[derive(Debug, Clone)]
pub struct UdtFieldPlan {
    /// The index of the origin field this target field takes its value from.
    pub origin_index: usize,
    /// How that value is converted.
    pub plan: ConversionPlan,
}

impl ConversionPlan {
    /// The plan kind, as `CDC-010` names it. Used in diagnostics and in the plan view a run
    /// exposes before it starts.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Passthrough => "Passthrough",
            Self::Codec(_) => "Codec",
            Self::Udt { .. } => "Udt",
            Self::List(_) => "List",
            Self::Set(_) => "Set",
            Self::Map { .. } => "Map",
            Self::Tuple(_) => "Tuple",
            Self::Vector { .. } => "Vector",
            Self::Unsupported { .. } => "Unsupported",
        }
    }

    /// Whether applying this plan is the identity on the serialised bytes, which is what makes the
    /// zero-copy fast path of `MIG-040` available.
    ///
    /// An `Unsupported` plan is identity too: `CDC-016` requires the value to pass through
    /// unchanged rather than fail the row.
    pub const fn is_identity(&self) -> bool {
        matches!(self, Self::Passthrough | Self::Unsupported { .. })
    }

    /// Converts one cell.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the value cannot be represented in the target
    /// type, or when the serialised value does not match the origin type. This is a record-level
    /// failure: the engine counts `ERROR`, logs the primary key and column, and continues.
    pub fn apply(&self, value: &RawCell) -> Result<RawCell, CdmError> {
        if self.is_identity() {
            return Ok(value.clone());
        }
        let Some(bytes) = value.bytes() else {
            return Ok(RawCell::NULL);
        };
        match self {
            Self::Codec(converter) => converter.convert(value),
            Self::List(element) => Ok(RawCell::new(self::apply_sequence(element, bytes, false)?)),
            Self::Set(element) => Ok(RawCell::new(self::apply_sequence(element, bytes, true)?)),
            Self::Map { key, value } => Ok(RawCell::new(self::apply_map(key, value, bytes)?)),
            Self::Tuple(elements) => Ok(RawCell::new(self::apply_tuple(elements, bytes)?)),
            Self::Udt { fields } => Ok(RawCell::new(self::apply_udt(fields, bytes)?)),
            Self::Vector {
                element,
                dimensions,
                origin_width,
            } => Ok(RawCell::new(self::apply_vector(
                element,
                *dimensions,
                *origin_width,
                bytes,
            )?)),
            Self::Passthrough | Self::Unsupported { .. } => Ok(value.clone()),
        }
    }
}

fn apply_element(
    plan: &ConversionPlan,
    element: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, CdmError> {
    match element {
        None => Ok(None),
        Some(bytes) => {
            let converted = plan.apply(&RawCell::new(bytes.to_vec()))?;
            Ok(converted.bytes().map(|b| b.to_vec()))
        }
    }
}

fn apply_sequence(plan: &ConversionPlan, bytes: &[u8], dedupe: bool) -> Result<Vec<u8>, CdmError> {
    let mut reader = Reader::new(bytes);
    let count = reader.take_i32()?;
    let mut converted: Vec<Option<Vec<u8>>> = Vec::new();
    for _ in 0..count.max(0) {
        let element = apply_element(plan, reader.take_element()?)?;
        // `Collectors.toSet()` in Java's convert_COLLECTION drops duplicates that the conversion
        // introduced; a CQL set with repeated elements is not a well-formed set.
        if dedupe && converted.contains(&element) {
            continue;
        }
        converted.push(element);
    }
    let count = i32::try_from(converted.len())
        .map_err(|_| conversion_error("collection has more than 2^31 elements"))?;
    let mut out = count.to_be_bytes().to_vec();
    for element in &converted {
        write_element(&mut out, element.as_deref())?;
    }
    Ok(out)
}

fn apply_map(
    key_plan: &ConversionPlan,
    value_plan: &ConversionPlan,
    bytes: &[u8],
) -> Result<Vec<u8>, CdmError> {
    let mut reader = Reader::new(bytes);
    let count = reader.take_i32()?;
    let mut out = count.to_be_bytes().to_vec();
    for _ in 0..count.max(0) {
        let key = apply_element(key_plan, reader.take_element()?)?;
        let value = apply_element(value_plan, reader.take_element()?)?;
        write_element(&mut out, key.as_deref())?;
        write_element(&mut out, value.as_deref())?;
    }
    Ok(out)
}

fn apply_tuple(plans: &[ConversionPlan], bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let mut reader = Reader::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    for plan in plans {
        // A tuple may be serialised with fewer components than the type declares; the missing
        // trailing ones are null.
        if reader.is_exhausted() {
            write_element(&mut out, None)?;
            continue;
        }
        let element = apply_element(plan, reader.take_element()?)?;
        write_element(&mut out, element.as_deref())?;
    }
    Ok(out)
}

fn apply_udt(fields: &[UdtFieldPlan], bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let mut reader = Reader::new(bytes);
    let mut origin: Vec<Option<Vec<u8>>> = Vec::with_capacity(fields.len());
    for _ in 0..fields.len() {
        if reader.is_exhausted() {
            origin.push(None);
            continue;
        }
        origin.push(reader.take_element()?.map(<[u8]>::to_vec));
    }
    let mut out = Vec::with_capacity(bytes.len());
    for field in fields {
        let value = origin.get(field.origin_index).ok_or_else(|| {
            conversion_error(format!(
                "UDT field index {} is out of range for a {}-field value",
                field.origin_index,
                origin.len()
            ))
        })?;
        let converted = apply_element(&field.plan, value.as_deref())?;
        write_element(&mut out, converted.as_deref())?;
    }
    Ok(out)
}

fn apply_vector(
    plan: &ConversionPlan,
    dimensions: usize,
    origin_width: usize,
    bytes: &[u8],
) -> Result<Vec<u8>, CdmError> {
    if bytes.len() != dimensions * origin_width {
        return Err(conversion_error(format!(
            "vector of {dimensions} elements should be {} bytes, got {}",
            dimensions * origin_width,
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut reader = Reader::new(bytes);
    for _ in 0..dimensions {
        let element = reader.take(origin_width)?;
        let converted = plan.apply(&RawCell::new(element.to_vec()))?;
        match converted.bytes() {
            Some(converted) => out.extend_from_slice(converted),
            None => return Err(conversion_error("a vector element cannot be null")),
        }
    }
    Ok(out)
}

/// How the planner should resolve the two places cdm-rs improves on Java (`CDC-014`, `CDC-015`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlannerOptions {
    /// Restore Java's behaviour exactly: match UDT fields by position only, and leave tuple
    /// elements unconverted. Set by the global `--compat-java` flag.
    pub compat_java: bool,
}

/// Resolves conversion plans (`CDC-010`).
#[derive(Debug, Clone, Default)]
pub struct Planner {
    registry: CodecRegistry,
    options: PlannerOptions,
}

/// One column's resolved plan, together with anything the operator should be told about it.
#[derive(Debug, Clone)]
pub struct ColumnPlan {
    column: String,
    origin: CqlTypeInfo,
    target: CqlTypeInfo,
    plan: ConversionPlan,
    diagnostics: Vec<Diagnostic>,
}

impl ColumnPlan {
    /// The column this plan converts.
    pub fn column(&self) -> &str {
        &self.column
    }

    /// The origin type.
    pub const fn origin(&self) -> &CqlTypeInfo {
        &self.origin
    }

    /// The target type.
    pub const fn target(&self) -> &CqlTypeInfo {
        &self.target
    }

    /// The plan itself.
    pub const fn plan(&self) -> &ConversionPlan {
        &self.plan
    }

    /// Warnings raised while planning, one per unsupported conversion (`CDC-016`). They are
    /// produced once, at startup, never per row.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for ColumnPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} -> {} [{}]",
            self.column,
            self.origin,
            self.target,
            self.plan.kind()
        )
    }
}

impl Planner {
    /// Creates a planner over a codec registry.
    pub const fn new(registry: CodecRegistry, options: PlannerOptions) -> Self {
        Self { registry, options }
    }

    /// The codec registry this planner resolves against.
    pub const fn registry(&self) -> &CodecRegistry {
        &self.registry
    }

    /// Plans one column, emitting the `CDC-016` warning for anything unsupported.
    ///
    /// The warning is logged here — at startup, once per column, naming the column and both types
    /// — and also returned, so a transport can surface it without scraping logs.
    pub fn plan_column(
        &self,
        column: &str,
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
    ) -> ColumnPlan {
        let plan = self.plan_types(origin, target);
        let mut diagnostics = Vec::new();
        collect_unsupported(&plan, &mut diagnostics, column);
        for diagnostic in &diagnostics {
            tracing::warn!(
                column = %column,
                origin = %origin,
                target = %target,
                "{}",
                diagnostic.title
            );
        }
        ColumnPlan {
            column: column.to_owned(),
            origin: origin.clone(),
            target: target.clone(),
            plan,
            diagnostics,
        }
    }

    /// Plans one type pair, without column context (`CDC-010`).
    pub fn plan_types(&self, origin: &CqlTypeInfo, target: &CqlTypeInfo) -> ConversionPlan {
        if origin.same_type(target) || assignable(origin, target) {
            return ConversionPlan::Passthrough;
        }
        match (origin, target) {
            (CqlTypeInfo::List { element: a, .. }, CqlTypeInfo::List { element: b, .. }) => {
                self.wrap(a, b, ConversionPlan::List)
            }
            (CqlTypeInfo::Set { element: a, .. }, CqlTypeInfo::Set { element: b, .. }) => {
                self.wrap(a, b, ConversionPlan::Set)
            }
            (
                CqlTypeInfo::Map {
                    key: ka, value: va, ..
                },
                CqlTypeInfo::Map {
                    key: kb, value: vb, ..
                },
            ) => self.plan_map(ka, va, kb, vb),
            (CqlTypeInfo::Tuple { elements: a }, CqlTypeInfo::Tuple { elements: b }) => {
                self.plan_tuple(a, b, origin, target)
            }
            (CqlTypeInfo::Udt { fields: a, .. }, CqlTypeInfo::Udt { fields: b, .. }) => {
                self.plan_udt(a, b, origin, target)
            }
            (
                CqlTypeInfo::Vector {
                    element: a,
                    dimensions: da,
                },
                CqlTypeInfo::Vector {
                    element: b,
                    dimensions: db,
                },
            ) => self.plan_vector(a, *da, b, *db, origin, target),
            _ if origin.is_primitive() && target.is_primitive() => {
                match self.registry.converter(origin, target) {
                    Some(converter) => ConversionPlan::Codec(Arc::clone(converter)),
                    None => unsupported(origin, target),
                }
            }
            // Cardinality mismatch (`map` to `list`, `list` to `udt`, …) is Unsupported
            // (`CDC-012`), as is any pair where only one side is a collection.
            _ => unsupported(origin, target),
        }
    }

    fn wrap(
        &self,
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
        build: fn(Box<ConversionPlan>) -> ConversionPlan,
    ) -> ConversionPlan {
        let element = self.plan_types(origin, target);
        if matches!(element, ConversionPlan::Passthrough) {
            return ConversionPlan::Passthrough;
        }
        build(Box::new(element))
    }

    fn plan_map(
        &self,
        origin_key: &CqlTypeInfo,
        origin_value: &CqlTypeInfo,
        target_key: &CqlTypeInfo,
        target_value: &CqlTypeInfo,
    ) -> ConversionPlan {
        let key = self.plan_types(origin_key, target_key);
        let value = self.plan_types(origin_value, target_value);
        if matches!(key, ConversionPlan::Passthrough)
            && matches!(value, ConversionPlan::Passthrough)
        {
            return ConversionPlan::Passthrough;
        }
        ConversionPlan::Map {
            key: Box::new(key),
            value: Box::new(value),
        }
    }

    fn plan_tuple(
        &self,
        origin_elements: &[CqlTypeInfo],
        target_elements: &[CqlTypeInfo],
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
    ) -> ConversionPlan {
        // CDC-015: Java leaves tuple elements unconverted; `--compat-java` restores that.
        if self.options.compat_java || origin_elements.len() != target_elements.len() {
            return unsupported(origin, target);
        }
        let plans: Vec<ConversionPlan> = origin_elements
            .iter()
            .zip(target_elements)
            .map(|(a, b)| self.plan_types(a, b))
            .collect();
        if plans
            .iter()
            .all(|p| matches!(p, ConversionPlan::Passthrough))
        {
            return ConversionPlan::Passthrough;
        }
        ConversionPlan::Tuple(plans)
    }

    fn plan_udt(
        &self,
        origin_fields: &[crate::types::UdtField],
        target_fields: &[crate::types::UdtField],
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
    ) -> ConversionPlan {
        // CDC-013: field counts must match.
        if origin_fields.is_empty() || origin_fields.len() != target_fields.len() {
            return unsupported(origin, target);
        }
        let by_name =
            !self.options.compat_java && names_match_as_sets(origin_fields, target_fields);
        let mut fields = Vec::with_capacity(target_fields.len());
        for (position, target_field) in target_fields.iter().enumerate() {
            let origin_index = if by_name {
                origin_fields
                    .iter()
                    .position(|f| f.name == target_field.name)
                    .unwrap_or(position)
            } else {
                position
            };
            let Some(origin_field) = origin_fields.get(origin_index) else {
                return unsupported(origin, target);
            };
            fields.push(UdtFieldPlan {
                origin_index,
                plan: self.plan_types(&origin_field.cql_type, &target_field.cql_type),
            });
        }
        if fields.iter().enumerate().all(|(position, field)| {
            field.origin_index == position && matches!(field.plan, ConversionPlan::Passthrough)
        }) {
            return ConversionPlan::Passthrough;
        }
        ConversionPlan::Udt { fields }
    }

    fn plan_vector(
        &self,
        origin_element: &CqlTypeInfo,
        origin_dimensions: usize,
        target_element: &CqlTypeInfo,
        target_dimensions: usize,
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
    ) -> ConversionPlan {
        if origin_dimensions != target_dimensions {
            return unsupported(origin, target);
        }
        let element = self.plan_types(origin_element, target_element);
        if matches!(element, ConversionPlan::Passthrough) {
            return ConversionPlan::Passthrough;
        }
        // A vector of a fixed-width element type is a contiguous array with no framing, which is
        // the only layout cdm-rs re-emits. A variable-width element type on either side would need
        // the length-prefixed layout, which `vector<float, N>` — the type `CDC-004` makes
        // first-class — never uses; such a pair converts as Unsupported (identity) rather than by
        // guessing at a framing.
        let (Some(origin_width), Some(_)) =
            (origin_element.fixed_width(), target_element.fixed_width())
        else {
            return unsupported(origin, target);
        };
        ConversionPlan::Vector {
            element: Box::new(element),
            dimensions: origin_dimensions,
            origin_width,
        }
    }
}

fn unsupported(origin: &CqlTypeInfo, target: &CqlTypeInfo) -> ConversionPlan {
    ConversionPlan::Unsupported {
        origin: Box::new(origin.clone()),
        target: Box::new(target.clone()),
    }
}

fn names_match_as_sets(
    origin: &[crate::types::UdtField],
    target: &[crate::types::UdtField],
) -> bool {
    let mut a: Vec<&str> = origin.iter().map(|f| f.name.as_str()).collect();
    let mut b: Vec<&str> = target.iter().map(|f| f.name.as_str()).collect();
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

/// Walks a plan, raising one `CDC-016` warning per unsupported leaf.
fn collect_unsupported(plan: &ConversionPlan, out: &mut Vec<Diagnostic>, column: &str) {
    match plan {
        ConversionPlan::Unsupported { origin, target } => out.push(
            Diagnostic::warning(
                ErrorKind::TypeConversion.diagnostic_code(),
                format!(
                    "column `{column}`: no conversion is known from {origin} to {target}; values \
                     will be written through unchanged"
                ),
            )
            .with_location(column.to_owned()),
        ),
        ConversionPlan::List(element)
        | ConversionPlan::Set(element)
        | ConversionPlan::Vector { element, .. } => collect_unsupported(element, out, column),
        ConversionPlan::Map { key, value } => {
            collect_unsupported(key, out, column);
            collect_unsupported(value, out, column);
        }
        ConversionPlan::Tuple(plans) => {
            for plan in plans {
                collect_unsupported(plan, out, column);
            }
        }
        ConversionPlan::Udt { fields } => {
            for field in fields {
                collect_unsupported(&field.plan, out, column);
            }
        }
        ConversionPlan::Passthrough | ConversionPlan::Codec(_) => {}
    }
}

/// Whether two different types have directly assignable representations, in which case the raw
/// bytes are already correct and the plan is `Passthrough` (`CDC-011`).
///
/// This mirrors Java's `calcConversionTypeForPrimitives`, which returns `NONE` when the target
/// codec's Java class is assignable from the origin codec's — `text`/`ascii` are both `String`,
/// `bigint`/`counter` are both `Long`, `uuid`/`timeuuid` are both `UUID` — and, like Java, it
/// performs no validation: a `text` value with non-ASCII bytes moved into an `ascii` column is
/// accepted here exactly as Java accepts it.
fn assignable(origin: &CqlTypeInfo, target: &CqlTypeInfo) -> bool {
    fn family(t: &CqlTypeInfo) -> Option<u8> {
        Some(match t {
            CqlTypeInfo::Text | CqlTypeInfo::Ascii => 0,
            CqlTypeInfo::BigInt | CqlTypeInfo::Counter => 1,
            CqlTypeInfo::Uuid | CqlTypeInfo::TimeUuid => 2,
            _ => return None,
        })
    }
    match (family(origin), family(target)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
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
    use crate::builtin::Codecset;
    use crate::types::UdtField;

    fn planner() -> Planner {
        Planner::new(
            CodecRegistry::with_builtins(&[Codecset::IntString, Codecset::BigintString], None)
                .unwrap(),
            PlannerOptions::default(),
        )
    }

    fn compat_planner() -> Planner {
        Planner::new(
            CodecRegistry::with_builtins(&[Codecset::IntString], None).unwrap(),
            PlannerOptions { compat_java: true },
        )
    }

    fn parse(text: &str) -> CqlTypeInfo {
        CqlTypeInfo::parse(text).unwrap()
    }

    fn udt(name: &str, fields: &[(&str, &str)]) -> CqlTypeInfo {
        CqlTypeInfo::Udt {
            keyspace: None,
            name: name.to_owned(),
            fields: fields
                .iter()
                .map(|(field, cql_type)| UdtField::new(*field, parse(cql_type)))
                .collect(),
            frozen: true,
        }
    }

    fn collection(elements: &[Option<&[u8]>]) -> Vec<u8> {
        let mut out = i32::try_from(elements.len())
            .unwrap()
            .to_be_bytes()
            .to_vec();
        for element in elements {
            write_element(&mut out, *element).unwrap();
        }
        out
    }

    fn fields(elements: &[Option<&[u8]>]) -> Vec<u8> {
        let mut out = Vec::new();
        for element in elements {
            write_element(&mut out, *element).unwrap();
        }
        out
    }

    #[test]
    fn cdc_010_the_plan_kinds_are_exactly_those_the_specification_names() {
        let planner = planner();
        let cases = [
            ("int", "int", "Passthrough"),
            ("int", "text", "Codec"),
            ("list<int>", "list<text>", "List"),
            ("set<int>", "set<text>", "Set"),
            ("map<int, int>", "map<int, text>", "Map"),
            ("tuple<int, int>", "tuple<int, text>", "Tuple"),
            ("vector<int, 3>", "vector<text, 3>", "Unsupported"),
            ("map<int, int>", "list<int>", "Unsupported"),
        ];
        for (origin, target, kind) in cases {
            let plan = planner.plan_types(&parse(origin), &parse(target));
            assert_eq!(plan.kind(), kind, "{origin} -> {target}");
        }
        let plan = planner.plan_types(&udt("a", &[("x", "int")]), &udt("b", &[("x", "text")]));
        assert_eq!(plan.kind(), "Udt");
        let plan = planner.plan_types(&parse("vector<int, 3>"), &parse("vector<bigint, 3>"));
        assert_eq!(plan.kind(), "Vector");
    }

    #[test]
    fn cdc_011_identical_and_assignable_types_are_passthrough() {
        let planner = planner();
        for (origin, target) in [
            ("int", "int"),
            ("frozen<list<int>>", "list<int>"),
            ("text", "varchar"),
            ("text", "ascii"),
            ("ascii", "text"),
            ("bigint", "counter"),
            ("uuid", "timeuuid"),
            ("map<text, frozen<list<int>>>", "map<text, list<int>>"),
        ] {
            let plan = planner.plan_types(&parse(origin), &parse(target));
            assert!(
                plan.is_identity(),
                "{origin} -> {target} was {}",
                plan.kind()
            );
            assert_eq!(plan.kind(), "Passthrough", "{origin} -> {target}");
        }
    }

    #[test]
    fn cdc_011_a_pair_with_no_registered_codec_is_unsupported() {
        // DOUBLE_STRING was not enabled, so double -> text has no codec.
        let plan = planner().plan_types(&CqlTypeInfo::Double, &CqlTypeInfo::Text);
        assert_eq!(plan.kind(), "Unsupported");
    }

    #[test]
    fn cdc_012_collections_recurse_and_cardinality_mismatch_is_unsupported() {
        let planner = planner();
        let plan = planner.plan_types(&parse("list<int>"), &parse("list<text>"));
        let value = RawCell::new(collection(&[Some(&[0, 0, 0, 10]), None]));
        let converted = plan.apply(&value).unwrap();
        assert_eq!(converted, RawCell::new(collection(&[Some(b"10"), None])));

        for (origin, target) in [
            ("map<int, int>", "list<int>"),
            ("list<int>", "set<int>"),
            ("list<int>", "int"),
            ("int", "list<int>"),
        ] {
            assert_eq!(
                planner.plan_types(&parse(origin), &parse(target)).kind(),
                "Unsupported",
                "{origin} -> {target}"
            );
        }
    }

    #[test]
    fn cdc_012_maps_convert_keys_and_values_independently() {
        let planner = planner();
        let plan = planner.plan_types(&parse("map<int, bigint>"), &parse("map<text, text>"));
        let entries = {
            let mut out = 1_i32.to_be_bytes().to_vec();
            write_element(&mut out, Some(&10_i32.to_be_bytes())).unwrap();
            write_element(&mut out, Some(&11_i64.to_be_bytes())).unwrap();
            out
        };
        let converted = plan.apply(&RawCell::new(entries)).unwrap();
        let expected = {
            let mut out = 1_i32.to_be_bytes().to_vec();
            write_element(&mut out, Some(b"10")).unwrap();
            write_element(&mut out, Some(b"11")).unwrap();
            out
        };
        assert_eq!(converted, RawCell::new(expected));
    }

    #[test]
    fn cdc_012_set_conversion_drops_duplicates_the_conversion_introduced() {
        let planner = planner();
        // Two distinct bigints that both render as the same text would be a real collision; here
        // the same value twice suffices to prove the set stays a set.
        let plan = planner.plan_types(&parse("set<int>"), &parse("set<text>"));
        let value = collection(&[Some(&[0, 0, 0, 10]), Some(&[0, 0, 0, 10])]);
        let converted = plan.apply(&RawCell::new(value)).unwrap();
        assert_eq!(converted, RawCell::new(collection(&[Some(b"10")])));
    }

    #[test]
    fn cdc_013_udt_conversion_requires_equal_field_counts() {
        let planner = planner();
        let plan = planner.plan_types(
            &udt("a", &[("x", "int"), ("y", "int")]),
            &udt("b", &[("x", "text")]),
        );
        assert_eq!(plan.kind(), "Unsupported");
        // An unresolved UDT — no field definitions — cannot be planned field-wise either.
        let plan = planner.plan_types(&parse("a_type"), &parse("b_type"));
        assert_eq!(plan.kind(), "Unsupported");
    }

    #[test]
    fn cdc_014_udt_fields_are_matched_by_name_when_the_name_sets_are_equal() {
        let planner = planner();
        let origin = udt("a", &[("x", "int"), ("y", "bigint")]);
        let target = udt("b", &[("y", "text"), ("x", "text")]);
        let ConversionPlan::Udt {
            fields: field_plans,
        } = planner.plan_types(&origin, &target)
        else {
            panic!("expected a Udt plan");
        };
        assert_eq!(
            field_plans[0].origin_index, 1,
            "target `y` takes origin `y`"
        );
        assert_eq!(
            field_plans[1].origin_index, 0,
            "target `x` takes origin `x`"
        );

        let value = fields_value();
        let converted = planner
            .plan_types(&origin, &target)
            .apply(&RawCell::new(value))
            .unwrap();
        assert_eq!(converted, RawCell::new(fields(&[Some(b"11"), Some(b"10")])));
    }

    fn fields_value() -> Vec<u8> {
        fields(&[Some(&10_i32.to_be_bytes()), Some(&11_i64.to_be_bytes())])
    }

    #[test]
    fn cdc_014_compat_java_restores_positional_udt_matching() {
        let planner = compat_planner();
        let origin = udt("a", &[("x", "int"), ("y", "int")]);
        let target = udt("b", &[("y", "text"), ("x", "text")]);
        let ConversionPlan::Udt {
            fields: field_plans,
        } = planner.plan_types(&origin, &target)
        else {
            panic!("expected a Udt plan");
        };
        assert_eq!(field_plans[0].origin_index, 0);
        assert_eq!(field_plans[1].origin_index, 1);
    }

    #[test]
    fn cdc_014_positional_matching_is_used_when_the_names_differ() {
        let planner = planner();
        let origin = udt("a", &[("x", "int")]);
        let target = udt("b", &[("renamed", "text")]);
        let ConversionPlan::Udt {
            fields: field_plans,
        } = planner.plan_types(&origin, &target)
        else {
            panic!("expected a Udt plan");
        };
        assert_eq!(field_plans[0].origin_index, 0);
    }

    #[test]
    fn cdc_014_a_udt_whose_fields_all_pass_through_in_order_is_itself_passthrough() {
        let planner = planner();
        let origin = udt("a", &[("x", "int"), ("y", "text")]);
        let target = udt("b", &[("x", "int"), ("y", "ascii")]);
        assert_eq!(planner.plan_types(&origin, &target).kind(), "Passthrough");
    }

    #[test]
    fn cdc_014_a_reordering_udt_plan_is_not_collapsed_to_passthrough() {
        let planner = planner();
        let origin = udt("a", &[("x", "int"), ("y", "int")]);
        let target = udt("b", &[("y", "int"), ("x", "int")]);
        assert_eq!(planner.plan_types(&origin, &target).kind(), "Udt");
    }

    #[test]
    fn cdc_015_tuple_elements_are_converted_and_compat_java_leaves_them_alone() {
        let planner = planner();
        let plan = planner.plan_types(&parse("tuple<int, int>"), &parse("tuple<text, int>"));
        assert_eq!(plan.kind(), "Tuple");
        let value = fields(&[Some(&10_i32.to_be_bytes()), Some(&11_i32.to_be_bytes())]);
        let converted = plan.apply(&RawCell::new(value.clone())).unwrap();
        assert_eq!(
            converted,
            RawCell::new(fields(&[Some(b"10"), Some(&11_i32.to_be_bytes())]))
        );

        // Java's CqlConversion has `// TODO: add TUPLE to this list`; --compat-java reproduces it.
        let compat =
            compat_planner().plan_types(&parse("tuple<int, int>"), &parse("tuple<text, int>"));
        assert_eq!(compat.kind(), "Unsupported");
        assert_eq!(
            compat.apply(&RawCell::new(value.clone())).unwrap(),
            RawCell::new(value)
        );
    }

    #[test]
    fn cdc_015_a_tuple_of_a_different_arity_is_unsupported() {
        let planner = planner();
        assert_eq!(
            planner
                .plan_types(&parse("tuple<int>"), &parse("tuple<int, int>"))
                .kind(),
            "Unsupported"
        );
    }

    #[test]
    fn cdc_015_missing_trailing_tuple_components_are_written_as_null() {
        let planner = planner();
        let plan = planner.plan_types(&parse("tuple<int, int>"), &parse("tuple<text, text>"));
        let value = fields(&[Some(&10_i32.to_be_bytes())]);
        let converted = plan.apply(&RawCell::new(value)).unwrap();
        assert_eq!(converted, RawCell::new(fields(&[Some(b"10"), None])));
    }

    #[test]
    fn cdc_004_vectors_convert_element_wise_over_a_contiguous_array() {
        let planner = planner();
        let plan = planner.plan_types(&parse("vector<int, 2>"), &parse("vector<bigint, 2>"));
        let ConversionPlan::Vector {
            dimensions,
            origin_width,
            ..
        } = &plan
        else {
            panic!("expected a Vector plan");
        };
        assert_eq!((*dimensions, *origin_width), (2, 4));

        // int -> bigint has no registered codec, so the element plan is Unsupported: identity.
        let mut value = 1_i32.to_be_bytes().to_vec();
        value.extend_from_slice(&2_i32.to_be_bytes());
        assert_eq!(
            plan.apply(&RawCell::new(value.clone())).unwrap(),
            RawCell::new(value)
        );
    }

    #[test]
    fn cdc_004_a_vector_of_a_different_arity_or_a_variable_width_element_is_unsupported() {
        let planner = planner();
        assert_eq!(
            planner
                .plan_types(&parse("vector<float, 3>"), &parse("vector<float, 4>"))
                .kind(),
            "Unsupported"
        );
        assert_eq!(
            planner
                .plan_types(&parse("vector<int, 3>"), &parse("vector<text, 3>"))
                .kind(),
            "Unsupported"
        );
        assert_eq!(
            planner
                .plan_types(&parse("vector<float, 3>"), &parse("vector<float, 3>"))
                .kind(),
            "Passthrough"
        );
    }

    #[test]
    fn cdc_004_a_vector_whose_length_disagrees_with_its_type_is_a_conversion_error() {
        let planner = planner();
        let plan = planner.plan_types(&parse("vector<int, 2>"), &parse("vector<bigint, 2>"));
        assert!(plan.apply(&RawCell::new(vec![0, 0, 0, 1])).is_err());
    }

    #[test]
    fn cdc_016_an_unsupported_plan_passes_values_through_and_warns_once_naming_the_column() {
        let planner = planner();
        let column = planner.plan_column("payload", &CqlTypeInfo::Double, &CqlTypeInfo::Text);
        assert_eq!(column.plan().kind(), "Unsupported");
        assert_eq!(
            column.diagnostics().len(),
            1,
            "one warning, not one per row"
        );
        let diagnostic = &column.diagnostics()[0];
        assert!(diagnostic.title.contains("payload"), "{diagnostic:?}");
        assert!(diagnostic.title.contains("double"), "{diagnostic:?}");
        assert!(diagnostic.title.contains("text"), "{diagnostic:?}");
        assert!(!diagnostic.is_blocking(), "a warning, not an error");
        assert_eq!(column.column(), "payload");
        assert_eq!(column.origin(), &CqlTypeInfo::Double);
        assert_eq!(column.target(), &CqlTypeInfo::Text);
        assert_eq!(column.to_string(), "payload: double -> text [Unsupported]");

        let value = RawCell::new(vec![1, 2, 3]);
        assert_eq!(column.plan().apply(&value).unwrap(), value);
    }

    #[test]
    fn cdc_016_a_nested_unsupported_element_is_warned_about_too() {
        let planner = planner();
        let column = planner.plan_column(
            "payload",
            &parse("map<int, double>"),
            &parse("map<text, text>"),
        );
        assert_eq!(column.plan().kind(), "Map");
        assert_eq!(column.diagnostics().len(), 1);
        let column = planner.plan_column("t", &parse("tuple<double>"), &parse("tuple<text>"));
        assert_eq!(column.diagnostics().len(), 1);
        let column = planner.plan_column(
            "u",
            &udt("a", &[("x", "double")]),
            &udt("b", &[("x", "text")]),
        );
        assert_eq!(column.diagnostics().len(), 1);
        let column = planner.plan_column("l", &parse("list<double>"), &parse("list<text>"));
        assert_eq!(column.diagnostics().len(), 1);
    }

    #[test]
    fn cdc_010_a_plan_is_resolved_once_and_then_applied_many_times() {
        let planner = planner();
        assert!(!planner.registry().is_empty());
        let plan = planner.plan_column("n", &CqlTypeInfo::Int, &CqlTypeInfo::Text);
        for value in 0..10_i32 {
            let converted = plan
                .plan()
                .apply(&RawCell::new(value.to_be_bytes().to_vec()))
                .unwrap();
            assert_eq!(converted, RawCell::new(value.to_string().into_bytes()));
        }
        assert_eq!(
            plan.plan().apply(&RawCell::NULL).unwrap(),
            RawCell::NULL,
            "MIG-012: null in, null out"
        );
    }

    #[test]
    fn cdc_010_a_malformed_serialised_collection_is_a_record_level_error() {
        let planner = planner();
        let plan = planner.plan_types(&parse("list<int>"), &parse("list<text>"));
        let error = plan.apply(&RawCell::new(vec![0, 0, 0, 2, 0])).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);
    }
}
